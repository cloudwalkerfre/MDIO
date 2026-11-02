// test/error-boundary.mjs —— 崩溃隔离回归（真实浏览器 + **应用同一份** React 实例）。
//
// 做法：在 app 页面里用原生 ESM 按页面 import map 的 URL 动态 import（⇒ 与运行中的应用共用模块实例），
// 把「必炸子组件」渲染进 <ErrorBoundary>，断言：降级卡出现 / 错误写进 logBus / resetKey 变化自动清错 / 宿主不受影响。
// 为什么这么测：边界只在**渲染期抛错**时触发，而应用里所有认领件都是防御式的（std 有兜底），
// 没有合法输入能让它抛错 ⇒ 用独立挂载点构造真错误，既不污染 src（判官 R6a 禁全局探针），也真跑 React 的捕获路径。
const CDP = process.env.CDP || "http://127.0.0.1:9222";
const BASE = process.env.BASE || "http://127.0.0.1:5178";
const SHOT_DIR = process.env.SHOT_DIR || ".tmp/steps-gui";
const fs = await import("node:fs");
const path = await import("node:path");
const REPO = path.resolve(import.meta.dirname, "../../..");     // demo/test → demo → mdio_ui → 仓库根（少一层会拼出 mdio_ui/mdio_ui）
// devkit 已下沉为库包（ui_devview）：Vite 把 `ui_devview/devkit/*` 解析成 `/demo/@fs<绝对路径>`。
// 探针必须走**同一个 URL**，才能拿到应用模块图里那份实例（否则 logBus 是第二个实例 —— 见下方 S1 注释）。
const CORE = "ui_core";   // 逻辑名（官方模型）。源码 URL 会另起模块图 ⇒ 探针拿到的是**另一个实例**（正是「两份 React」的来源）。
// logBus 已按"呈现关切"搬进 devview：日志面板的数据源与面板同目录；
// 框架侧只留无缓冲的监控出口 core/services/monitor.js，装配点用 monitorSink(log) 接线。
const DEVVIEW = "ui_devview";   // 同上：逻辑名（与应用同一份实例）

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
const ev = (expr, awaitP = false) => send("Runtime.evaluate", { expression: expr, returnByValue: true, awaitPromise: awaitP }).then((r) => r.result && r.result.value);
const say = (m) => process.stderr.write("[eb] " + m + "\n");
say("start");
const R = [];
const chk = (cid, name, ok, detail) => R.push({ cid, name, ok: !!ok, detail: detail === undefined ? "" : String(detail).slice(0, 220) });

say("page=" + (pg && pg.id ? "ok" : "none"));
let booted = false;
// 等待条件从"文件树出现了"升级为"**import map 里 react-dom/client 已就位**"：
// 由来（devkit 下沉首跑实测）：新包首次被请求时 Vite 会做依赖预构建并**整页重载**，
// 旧条件在重载窗口里就为真 ⇒ 探针 `import(map["react"])` 拿到 undefined ⇒ 抛错，表现为四点全红而页面其实好的。
const IMAP = "JSON.parse(document.querySelector('script[type=importmap]').textContent).imports";
for (let i = 0; i < 60; i += 1) {
  await wait(900);
  const ok = await ev("!!document.querySelector('.dv-ft-file') && !!(" + IMAP + ")['react-dom/client']");
  if (ok) { booted = true; break; }
}
await wait(2500);
say("booted=" + booted);

// ① 挂上「必炸子组件」，并读回降级卡的四件事实（保留挂载点以便截图，先不恢复）
const S1 = `(async () => { try {
  const map = JSON.parse(document.querySelector('script[type=importmap]').textContent).imports || {};
  const R = await import(map["react"]);
  const { createRoot } = await import(map["react-dom/client"]);
  const { default: ErrorBoundary } = await import("${CORE}/ErrorBoundary");
  const { logSnapshot } = await import("${DEVVIEW}/logBus");
  const host = document.createElement("div");
  host.id = "eb-host";
  host.style.cssText = "position:fixed;left:430px;top:150px;width:460px;z-index:9999";
  document.body.appendChild(host);
  const root = createRoot(host);
  const Boom = () => { throw new Error("EB_TEST_BOOM：某个认领件渲染期抛错"); };
  root.render(R.createElement(ErrorBoundary, { name: "节点 tb", resetKey: "v1" }, R.createElement(Boom)));
  await new Promise((r) => setTimeout(r, 500));
  const q = (sel) => host.querySelector(sel);
  // ⚠️ 成因（2026-10-01 实测 · task-25）：本探针**硬编码** devkit 的 URL（当时是 /demo/src/std/devkit/logBus.js；
  //    框架下沉后由上方 CORE 常量派生），而应用模块图里同一模块在 HMR 之后带版本戳 ⇒ 拿到的是**另一个实例**：
  //    探针 logSnapshot().entries 只有 1 条（探针自己手动 log 的），
  //    而页面 .dv-log-line 里确实有 "[…] EB✗: 节点 tb 渲染崩溃: EB_TEST_BOOM…"（已实证）。所以判据读**页面日志面板（DOM）**——
  //    那才是"写进 logBus（日志面板可见）"的原文语义；logBus 实例读数保留为诊断字段（loggedBus），不再是唯一判据。
  const loggedBus = (logSnapshot().entries || []).some((e) => String(e.text || "").indexOf("EB✗: 节点 tb") >= 0);
  const loggedDom = [].slice.call(document.querySelectorAll(".dv-log-line")).some((l) => String(l.innerText || "").indexOf("EB✗: 节点 tb") >= 0);
  const logged = loggedDom || loggedBus;
  const appAlive = !!document.querySelector(".dv-root") && !!document.querySelector(".dv-footer") && !!document.querySelector(".dv-ft-file");
  window.__ebRoot = root;
  return JSON.stringify({ fallback: !!q(".dv-eb"), ttl: (q(".dv-eb-ttl") || {}).textContent || "", msg: (q(".dv-eb-msg") || {}).textContent || "", hasRetry: !!q(".dv-eb button"), logged, loggedBus, loggedDom, appAlive });
} catch (e) { return JSON.stringify({ err: String((e && e.message) || e) }); } })()`;
let a = {};
try { a = JSON.parse(String((await ev(S1, true)) || "{}")); } catch (e) { a = { parse: String(e.message).slice(0, 80) }; }

// ② 截图（降级卡可见态）
try { fs.mkdirSync(SHOT_DIR, { recursive: true }); } catch (e) { /* ignore */ }
say("S1 done " + JSON.stringify(a).slice(0, 120));
const shot = await send("Page.captureScreenshot", { format: "png" });
if (shot && shot.data) fs.writeFileSync(SHOT_DIR + "/20-error-boundary.png", Buffer.from(shot.data, "base64"));

say("shot=" + (shot && shot.data ? shot.data.length : 0));
// ③ 内容版本变（resetKey v1→v2）且换成不炸的子件 ⇒ 自动清错
const S2 = `(async () => { try {
  const map = JSON.parse(document.querySelector('script[type=importmap]').textContent).imports || {};
  const R = await import(map["react"]);
  const { default: ErrorBoundary } = await import("${CORE}/ErrorBoundary");
  const host = document.getElementById("eb-host");
  window.__ebRoot.render(R.createElement(ErrorBoundary, { name: "节点 tb", resetKey: "v2" }, R.createElement("div", { id: "eb-ok" }, "OK")));
  await new Promise((r) => setTimeout(r, 500));
  const done = !!host.querySelector("#eb-ok") && !host.querySelector(".dv-eb");
  host.remove();
  return JSON.stringify({ recovered: done });
} catch (e) { return JSON.stringify({ err: String((e && e.message) || e) }); } })()`;
let b = {};
try { b = JSON.parse(String((await ev(S2, true)) || "{}")); } catch (e) { b = { parse: String(e.message).slice(0, 80) }; }

say("S2 done " + JSON.stringify(b));
chk("EB1", "渲染期抛错 ⇒ 该区域降级成错误卡（含名称/原文/重试）", booted && a.fallback === true && /已隔离/.test(a.ttl || "") && /EB_TEST_BOOM/.test(a.msg || "") && a.hasRetry === true, JSON.stringify(a).slice(0, 200));
chk("EB2", "错误同时写进 logBus（日志面板可见）", a.logged === true, JSON.stringify({ logged: a.logged, loggedDom: a.loggedDom, loggedBus: a.loggedBus }));
chk("EB3", "resetKey（内容版本）变化 ⇒ 自动清错，无需手点重试", b.recovered === true, JSON.stringify(b));
chk("EB4", "隔离生效：宿主其余区域（文件树/输入条/根节点）不受影响", a.appAlive === true, JSON.stringify({ appAlive: a.appAlive }));

const failed = R.filter((x) => !x.ok);
for (const x of R) console.log((x.ok ? " ✓ " : " ✗ ") + x.cid + " " + x.name + (x.ok ? "" : "  → " + x.detail));
console.log(failed.length ? "✗ error-boundary 失败 " + failed.length + " 项" : "✓ error-boundary 全绿（" + R.length + " 项）· 截图见 " + SHOT_DIR);
try { await fetch(CDP + "/json/close/" + pg.id); } catch (e) { /* ignore */ }
process.exit(failed.length ? 1 : 0);