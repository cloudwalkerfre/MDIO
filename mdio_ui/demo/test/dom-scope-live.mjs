// dom-scope-live.mjs —— 「DOM/CSS 隔离原语真的在起作用」的活体判据（真页面 · 自建无头 9223）。
// 验三件：① app 那一格（#app）= 布局/绘制/样式容器 + 独立层叠上下文；② `@scope (#app)` 只圈住 app 那一格；
//        ③ 影子根是**双向硬边界**（采纳进影子根的样式只作用里面；文档级样式进不去）。
// 跑法：node mdio_ui/demo/test/dom-scope-live.mjs
import { installGuards, makeEv, armWatchdog, guardTarget, withCallTimeout } from "./lib/watchdog.mjs";   // 超时 / 卡死检测 / 目标守卫（9222 = 业主 Chrome，默认拒绝）
const CDP = process.env.CDP || "http://127.0.0.1:9223";
guardTarget(CDP, "dom-scope-live");   // 连 9222（业主 Chrome）直接拒绝
armWatchdog("dom-scope-live", 10000)   /* = 10s */;   // 实测 max=1s（本机 2 跑）⇒ ×2=2.0s，取下限 10s ⇒ 10s
installGuards("dom-scope-live");   // 未捕获异常 / 未处理拒绝 ⇒ 打印并 exit 4（不留「跑不完」）
const BASE = process.env.BASE || "http://127.0.0.1:5178";
const WT = process.env.WT || "czUR8zxu";
const DOC = process.env.DOC || "/scripts/剧本.md";
let pass = 0, total = 0;
const chk = (n, ok, d) => { total += 1; if (ok) pass += 1; console.log((ok ? "  ok   " : "  FAIL ") + n + (d ? "  — " + String(d).slice(0, 240) : "")); };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const url = BASE + "/?wt=" + WT + "&app=" + encodeURIComponent(DOC);

const tab = await (await fetch(CDP + "/json/new?" + encodeURIComponent(url), { method: "PUT" })).json();
const ws = new WebSocket(tab.webSocketDebuggerUrl);
let seq = 0; const pend = new Map(); const errs = [];
ws.addEventListener("message", (e) => {
  try { const m = JSON.parse(e.data);
    if (m.id && pend.has(m.id)) { pend.get(m.id)(m.result || {}); pend.delete(m.id); return; }
    if (m.method === "Runtime.exceptionThrown") errs.push(String((m.params.exceptionDetails.exception || {}).description || "").slice(0, 160));
  } catch (e2) { /* ignore */ }
});
await new Promise((r) => ws.addEventListener("open", r));
const rawSend = (method, params) => new Promise((r) => { const i = ++seq; pend.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
const send = withCallTimeout("dom-scope-live", rawSend);   // 单次 CDP 调用 20s 超时 ⇒ 挂起会抛异常而不是永远等
await send("Runtime.enable", {});
const pageErrs = [];   // 页面异常账（Runtime.evaluate 的 exceptionDetails）
const ev = makeEv(send, "dom-scope-live", pageErrs);
for (let i = 0; i < 60; i += 1) { if ((await ev("((document.getElementById('app')||{}).shadowRoot||document).querySelectorAll('[data-app-list] a').length>=2")) === true) break; await sleep(600); }

// ① 容器三件套（返回对象，不再在页面里套 JSON.stringify）
{
  const st = await ev("(() => { const e = document.getElementById('app'); if (!e) return { missing: true }; const cs = getComputedStyle(e); return { contain: cs.contain, containerType: cs.containerType, containerName: cs.containerName, isolation: cs.isolation }; })()");
  const s = st && typeof st === "object" ? st : {};
  chk("① #app 在（appview 那一格）", s.missing !== true, JSON.stringify(s));
  // 注：`contain: layout paint style` 的三件套**等价于**关键字 `content`，浏览器计算值序列化成 `content`（实测）⇒ 两种写法都算通过。
  const hasContain = ["content", "strict"].indexOf(String(s.contain || "")) >= 0 || (String(s.contain || "").indexOf("layout") >= 0 && String(s.contain || "").indexOf("paint") >= 0 && String(s.contain || "").indexOf("style") >= 0);
  chk("① contain = layout+paint+style（内部布局与外部解耦）", hasContain, JSON.stringify(s));
  chk("① container-type = inline-size + 命名 appview", s.containerType === "inline-size" && s.containerName === "appview", JSON.stringify(s));
  chk("① 独立层叠上下文（boundary sheet：isolation: isolate）", s.isolation === "isolate", JSON.stringify(s));
}
// ② @scope (#app) 的作用域
{
  const js = [
    "(async () => {",
    "  const m = await import('ui_core/domScope');",
    "  const sr = (document.getElementById('app')||{}).shadowRoot;",
    "  const probe = new CSSStyleSheet();",
    "  // 有影子根（B 路线）⇒ 样式必须**采纳进影子根**（@scope/属性前缀法都到不了影子树）；没有影子根则走 @scope (#app)。",
    "  probe.replaceSync(sr ? '.iso-probe{color:rgb(1,2,3)}' : m.atScopeText('.iso-probe{color:rgb(1,2,3)}', '#app'));",
    "  const root2 = sr || document;",
    "  root2.adoptedStyleSheets = [...root2.adoptedStyleSheets, probe];",
    "  const inside = document.createElement('span'); inside.className = 'iso-probe';",
    "  const outside = document.createElement('span'); outside.className = 'iso-probe';",
    "  (sr || document.getElementById('app')).appendChild(inside);",
    "  document.body.appendChild(outside);",
    "  const ci = getComputedStyle(inside).color, co = getComputedStyle(outside).color;",
    "  const hasKey = !!m.atScopeText && !!m.ensureIsolatedRoot && !!m.adoptInto && !!m.scopeCssByAttr && !!m.namespaceKeyframes && !!m.globalGuard;",
    "  inside.remove(); outside.remove();",
    "  root2.adoptedStyleSheets = root2.adoptedStyleSheets.filter((s) => s !== probe);",
    "  return { hasKey: hasKey, inside: ci, outside: co };",
    "})()",
  ].join(String.fromCharCode(10));
  const st = await ev(js);
  const s = st && typeof st === "object" ? st : {};
  chk("② ui_core/domScope 原语齐（@scope + 影子根 + guard 三件，页面里能 import）", s.hasKey === true, JSON.stringify(s));
  chk("② 样式世界：里面生效（无影子根=@scope(#app)，有影子根=采纳进影子根）", String(s.inside || "").indexOf("1, 2, 3") >= 0, JSON.stringify(s));
  chk("② 样式世界：**外面不受影响**", String(s.outside || "").indexOf("1, 2, 3") < 0, JSON.stringify(s));
}
// ③ 影子根 = 双向硬边界
{
  const js = [
    "(async () => {",
    "  const m = await import('ui_core/domScope');",
    "  const host = document.createElement('div');",
    "  host.style.cssText = 'position:fixed;left:-9999px;top:0';",
    "  document.body.appendChild(host);",
    "  const root = m.ensureIsolatedRoot(host);",
    "  const sheet = new CSSStyleSheet(); sheet.replaceSync('.iso-shadow{color:rgb(4,5,6)}');",
    "  m.adoptInto(root, sheet);",
    "  const docSheet = new CSSStyleSheet(); docSheet.replaceSync('.iso-doc{color:rgb(7,8,9)}');",
    "  document.adoptedStyleSheets = [...document.adoptedStyleSheets, docSheet];",
    "  const inEl = document.createElement('span'); inEl.className = 'iso-shadow iso-doc';",
    "  root.appendChild(inEl);",
    "  const outEl = document.createElement('span'); outEl.className = 'iso-shadow';",
    "  document.body.appendChild(outEl);",
    "  const cin = getComputedStyle(inEl).color, cout = getComputedStyle(outEl).color;",
    "  const shadowExists = !!host.shadowRoot;",
    "  host.remove(); outEl.remove();",
    "  document.adoptedStyleSheets = document.adoptedStyleSheets.filter((s) => s !== docSheet);",
    "  return { shadowExists: shadowExists, inside: cin, outside: cout };",
    "})()",
  ].join(String.fromCharCode(10));
  const st = await ev(js);
  const s = st && typeof st === "object" ? st : {};
  chk("③ 影子根建立（硬边界存在）", s.shadowExists === true, JSON.stringify(s));
  chk("③ 采纳进影子根的样式：里面生效", String(s.inside || "").indexOf("4, 5, 6") >= 0, JSON.stringify(s));
  chk("③ 同一份样式：**外面不受影响**", String(s.outside || "").indexOf("4, 5, 6") < 0, JSON.stringify(s));
  chk("③ 文档级样式**进不去**影子根（双向隔离）", String(s.inside || "").indexOf("7, 8, 9") < 0, JSON.stringify(s));
}
// ④ 属性前缀法（@esmap/guard 那条路：不依赖 @scope，全浏览器可用）
{
  const js = [
    "(async () => {",
    "  const m = await import('ui_core/domScope');",
    "  const css = m.scopeCssByAttr('.iso-attr{color:rgb(11,12,13)}', 'iso');",   // guard 签名：(css, prefix)，属性名固定 data-esmap-scope
    "  const sheet = new CSSStyleSheet(); sheet.replaceSync(css);",
    "  document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];",
    "  const box = document.createElement('div'); box.setAttribute('data-esmap-scope', 'iso'); document.body.appendChild(box);",
    "  const inside = document.createElement('span'); inside.className = 'iso-attr'; box.appendChild(inside);",
    "  const outside = document.createElement('span'); outside.className = 'iso-attr'; document.body.appendChild(outside);",
    "  const ci = getComputedStyle(inside).color, co = getComputedStyle(outside).color;",
    "  box.remove(); outside.remove();",
    "  document.adoptedStyleSheets = document.adoptedStyleSheets.filter((s) => s !== sheet);",
    "  return { css: css, inside: ci, outside: co };",
    "})()",
  ].join(String.fromCharCode(10));
  const st = await ev(js);
  const s = st && typeof st === "object" ? st : {};
  chk("④ 属性前缀法（guard）：文本被前缀成 [data-esmap-scope=iso] …", String(s.css || "").indexOf("[data-esmap-scope=\"iso\"] .iso-attr") >= 0, JSON.stringify({ css: String(s.css || "").slice(0, 90) }));
  chk("④ 属性前缀法：里面生效", String(s.inside || "").indexOf("11, 12, 13") >= 0, JSON.stringify(s));
  chk("④ 属性前缀法：外面不受影响", String(s.outside || "").indexOf("11, 12, 13") < 0, JSON.stringify(s));
}
// ⑤ @keyframes 必须自己改名（@scope 不会局部化 keyframes ⇒ 两条 app 的 spin 会互相覆盖）
{
  const js = [
    "(async () => {",
    "  const m = await import('ui_core/domScope');",
    "  const src = '@keyframes spin{from{opacity:0}} .k{animation: spin 1s linear} .k2{animation-name:spin}';",
    "  const out = m.namespaceKeyframes(src, 'x');",
    "  const m2 = /@keyframes\\s+([A-Za-z_][\\w-]*)/.exec(out);",
    "  const renamed = m2 ? m2[1] : '';",
    "  return { out: out, renamed: renamed, bare: /@keyframes\\s+spin\\b/.test(out) };",
    "})()",
  ].join(String.fromCharCode(10));
  const st = await ev(js);
  const out = String((st && st.out) || "");
  const renamed = String((st && st.renamed) || "");
  chk("⑤ keyframes 改名：不再有裸 `@keyframes spin`（guard 会加前缀/净化）", (st && st.bare) === false && renamed.length > 0 && renamed !== "spin", "renamed=" + renamed + " | " + out.slice(0, 110));
  const refOk = out.indexOf("animation: " + renamed) >= 0 && new RegExp("animation-name:\\s*" + renamed).test(out);
  chk("⑤ keyframes 改名：引用一起改到同一个新名字", refOk, out.slice(0, 160));
}
// ⑥ 全局污染守卫（轮询 + 快照差集，不用 Proxy）
{
  const js = [
    "(async () => {",
    "  const m = await import('ui_core/domScope');",
    "  const hits = [];",
    "  const g = m.globalGuard({ interval: 30, onViolation: (k) => hits.push(typeof k === 'string' ? k : (k && k.property)) });",   // guard 回调给的是对象 {property,type}
    "  globalThis.__iso_probe_global = 1;",
    "  await new Promise((r) => setTimeout(r, 160));",
    "  const seen = (typeof g.dispose === 'function') ? g.dispose() : [];",
    "  delete globalThis.__iso_probe_global;",
    "  return { hits: hits, seen: seen };",
    "})()",
  ].join(String.fromCharCode(10));
  const st = await ev(js);
  const s = st && typeof st === "object" ? st : {};
  chk("⑥ 全局守卫：抓到新塞进 window 的键", Array.isArray(s.hits) && s.hits.indexOf("__iso_probe_global") >= 0, JSON.stringify(s));
}
// ⑦ guard 的属性前缀法真的接到了 app 那一格（边界 sheet 走 guard，`#app` 带作用域属性）
{
  const st = await ev("(() => { const e = document.getElementById('app'); if (!e) return { missing: true }; const cs = getComputedStyle(e); return { attr: e.getAttribute('data-esmap-scope'), isolation: cs.isolation }; })()");
  const s = st && typeof st === "object" ? st : {};
  chk("⑦ #app 带 guard 的作用域属性（data-esmap-scope）", s.attr === "mdio-app", JSON.stringify(s));
  chk("⑦ 边界 sheet 由 guard 采纳（属性法）⇒ isolation: isolate 生效", s.isolation === "isolate", JSON.stringify(s));
}
chk("⑧ 无未捕获异常", errs.length === 0, JSON.stringify(errs.slice(0, 2)));
chk("无页面异常（Runtime.evaluate 的 exceptionDetails）", pageErrs.length === 0, pageErrs.slice(0, 2).join(" | "));
console.log((pass === total ? "✓ DOM/CSS 隔离判据全绿（" : "✗ 失败 ") + pass + "/" + total + (pass === total ? "）" : ""));
await fetch(CDP + "/json/close/" + tab.id).catch(() => {});
process.exit(pass === total ? 0 : 1);
