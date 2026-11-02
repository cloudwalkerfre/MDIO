// test/mst-recursion.mjs —— 把「递归 React 函数组件 + MST/observer」的机制钉成回归测试。
//
// 背景（见 .tmp/sop/MST-REACT-RESEARCH.md §3 / §6 / §13）：三处独立来源（mobx#3836 具名函数自绑定、
// MST#1718 维护者答、SO#70568283 采纳答案）+ 源码机制都指向同一条规则 —— **递归调用点必须引用 observer 包装后的绑定**，
// 否则递归子层既没有 per-instance Reaction（不订阅任何东西），也没有 memo。
//
// 本测试在页面里建三棵等价树（root + 3 个孩子，key 用稳定 id），只改「第 2 个孩子」的文本，断言：
//   A 正确写法（匿名箭头）          ⇒ 只有该孩子重渲；父与兄弟都不动（节点级粒度）
//   B 具名函数表达式自绑定          ⇒ **静默失效**：什么都不重渲、DOM 停在旧值
//   C observer 只加在导出处          ⇒ 同 B
// 若哪天 B/C 变成"会重渲"，说明上游语义变了 ⇒ 本测试失败并提醒重读源码。
//
// 前置：esmx dev（http://127.0.0.1:5178）与 headless Chrome（:9222）在跑。
const CDP = process.env.CDP || "http://127.0.0.1:9222";
const BASE = process.env.BASE || "http://127.0.0.1:5178";
const pg = await (await fetch(CDP + "/json/new?" + encodeURIComponent(BASE + "/"), { method: "PUT" })).json();
const ws = new WebSocket(pg.webSocketDebuggerUrl);
let id = 0; const pend = new Map(); const logs = [];
const send = (m, pa) => new Promise((res) => { const i = ++id; pend.set(i, res); try { ws.send(JSON.stringify({ id: i, method: m, params: pa })); } catch (e) { res({}); } });
ws.addEventListener("message", (ev) => { try { const m = JSON.parse(ev.data);
  if (m.id && pend.has(m.id)) { pend.get(m.id)(m.result || {}); pend.delete(m.id); return; }
  if (m.method === "Runtime.consoleAPICalled") logs.push(String(m.params.args.map((a) => String(a.value != null ? a.value : (a.description || ""))).join(" ")));
} catch (e) {} });
await new Promise((r) => ws.addEventListener("open", r));
await send("Runtime.enable"); await send("Page.enable");
await new Promise((r) => setTimeout(r, 4000));
// 依赖**走裸标识符**（页面 import map → 宿主 shim ⇒ 与 app 同一实例）：早先这里硬编码了 Vite 预打包
// 的 URL + 哈希（/demo/node_modules/.vite/deps/react.js?v=…）⇒ 依赖一被重新预打包，哈希过时、动态 import 504，
// 测试红得莫名其妙（实测）。裸名由 import map 解析，不依赖预打包哈希。
const PAGE = "(async () => {\n  const out = {};\n  try {\n    const Rns = await import(\"react\");\n    const R = typeof Rns.memo === \"function\" ? Rns : (Rns.default || Rns);\n    const RDns = await import(\"react-dom/client\");\n    const RD = typeof RDns.createRoot === \"function\" ? RDns : (RDns.default || RDns);\n    const MRLns = await import(\"mobx-react-lite\");\n    const MRL = typeof MRLns.observer === \"function\" ? MRLns : (MRLns.default || MRLns);\n    const MSTns = await import(\"mobx-state-tree\");\n    const MST = typeof MSTns.types === \"object\" ? MSTns : (MSTns.default || MSTns);\n    const h = R.createElement;\n    const wait = (ms) => new Promise((r) => setTimeout(r, ms));\n    const N = MST.types.model(\"RN\", { id: MST.types.string, t: MST.types.optional(MST.types.string, \"\"), ct: MST.types.optional(MST.types.array(MST.types.late(() => N)), () => []) });\n    const Root = MST.types.model(\"RRoot\", { ct: MST.types.array(N) });\n    const mk = () => Root.create({ ct: [{ id: \"r\", t: \"root\", ct: [{ id: \"a\", t: \"a\" }, { id: \"b\", t: \"b\" }, { id: \"c\", t: \"c\" }] }] });\n    const mkC = () => ({ r: 0, a: 0, b: 0, c: 0 });\n    const cA = mkC(); const cB = mkC(); const cC = mkC();\n    const A = MRL.observer(({ node }) => { cA[node.id] += 1; return h(\"div\", null, node.t, node.ct.map((k) => h(A, { key: k.id, node: k }))); });\n    const B = MRL.observer(function B({ node }) { cB[node.id] += 1; return h(\"div\", null, node.t, node.ct.map((k) => h(B, { key: k.id, node: k }))); });\n    const CBody = ({ node }) => { cC[node.id] += 1; return h(\"div\", null, node.t, node.ct.map((k) => h(CBody, { key: k.id, node: k }))); };\n    const C = MRL.observer(CBody);\n    const hosts = {};\n    for (const [name, Comp, store] of [[\"A\", A, mk()], [\"B\", B, mk()], [\"C\", C, mk()]]) {\n      const el = document.createElement(\"div\"); document.body.appendChild(el);\n      RD.createRoot(el).render(h(Comp, { node: store.ct[0] }));\n      hosts[name] = { el, store };\n    }\n    await wait(500);\n    const before = { A: { ...cA }, B: { ...cB }, C: { ...cC } };\n    for (const name of [\"A\", \"B\", \"C\"]) MST.applyPatch(hosts[name].store, [{ op: \"replace\", path: \"/ct/0/ct/1/t\", value: \"b2\" }]);\n    await wait(400);\n    const d = (cur, b) => ({ r: cur.r - b.r, a: cur.a - b.a, b: cur.b - b.b, c: cur.c - b.c });\n    out.A = d(cA, before.A); out.B = d(cB, before.B); out.C = d(cC, before.C);\n    out.domA = hosts.A.el.innerText.replace(/\\n/g, \"|\");\n    out.domB = hosts.B.el.innerText.replace(/\\n/g, \"|\");\n    out.domC = hosts.C.el.innerText.replace(/\\n/g, \"|\");\n    out.ok = true;\n  } catch (e) { out.err = String(e && e.message); }\n  console.log(\"[rec] \" + JSON.stringify(out));\n})()";
await send("Runtime.evaluate", { expression: PAGE, awaitPromise: true, returnByValue: true });
await new Promise((r) => setTimeout(r, 1500));
const line = logs.filter((l) => l.indexOf("[rec]") === 0).slice(-1)[0] || "";
const R = [];
const chk = (cid, name, ok, detail) => R.push({ id: cid, name, ok: !!ok, detail: detail === undefined ? "" : String(detail) });
let res = null;
try { res = JSON.parse(line.slice(6)); } catch (e) { /* keep null */ }
chk("R1", "页面脚本执行", !!res && res.ok === true, (res && res.err) ? res.err : line.slice(0, 120));
if (res && res.A) {
  chk("R2", "A 正确写法：只有被改的孩子重渲", res.A.b === 1 && res.A.a === 0 && res.A.c === 0, JSON.stringify(res.A));
  chk("R3", "A 正确写法：父不因子的变化重渲", res.A.r === 0, JSON.stringify(res.A));
  chk("R4", "A：DOM 已更新", String(res.domA).indexOf("b2") >= 0, res.domA);
  const bSilent = res.B.b === 0 && res.B.r === 0 && res.B.a === 0 && res.B.c === 0;
  chk("R5", "B 具名函数自绑定：静默失效（零重渲）", bSilent, JSON.stringify(res.B));
  chk("R6", "B：DOM 停在旧值（无 b2）", String(res.domB).indexOf("b2") < 0, res.domB);
  const cSilent = res.C.b === 0 && res.C.r === 0 && res.C.a === 0 && res.C.c === 0;
  chk("R7", "C observer 只在导出处：静默失效（零重渲）", cSilent, JSON.stringify(res.C));
  chk("R8", "C：DOM 停在旧值（无 b2）", String(res.domC).indexOf("b2") < 0, res.domC);
  chk("R9", "对照：A 命中而 B/C 未命中", res.A.b > 0 && res.B.b === 0 && res.C.b === 0, JSON.stringify({ A: res.A.b, B: res.B.b, C: res.C.b }));
}
const failed = R.filter((r) => !r.ok);
for (const r of R) console.log((r.ok ? " ✓ " : " ✗ ") + r.id + " " + r.name + (r.ok ? "" : "  → " + r.detail));
console.log(failed.length ? "✗ mst-recursion 失败 " + failed.length + " 项" : "✓ mst-recursion 全绿（" + R.length + " 项）");
try { await fetch(CDP + "/json/close/" + pg.id); } catch (e) {}
process.exit(failed.length ? 1 : 0);
