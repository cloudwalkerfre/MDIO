// CDP 探针（一次性证据脚本，跑完即弃）：验证浏览器侧 ui:std.js 真能 import + 认领组件真加载
const CDP = "http://127.0.0.1:9222";
const PAGE = "http://127.0.0.1:5178/";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let id = 0;
const pending = new Map();
const consoleMsgs = [];
const netFails = [];
let ws;
async function send(method, params) {
  return new Promise((res, rej) => { const i = ++id; pending.set(i, { res, rej }); ws.send(JSON.stringify({ id: i, method, params })); });
}
async function evalJs(expression, awaitPromise = false) {
  const r = await send("Runtime.evaluate", { expression, awaitPromise, returnByValue: true, userGesture: true });
  if (r.exceptionDetails) throw new Error("eval: " + JSON.stringify(r.exceptionDetails).slice(0, 300));
  return r.result.value;
}
const list = await (await fetch(CDP + "/json/list")).json();
const target = list.find((t) => t.type === "page");
if (!target) throw new Error("no page target");
ws = new WebSocket(target.webSocketDebuggerUrl);
ws.addEventListener("message", (ev) => {
  const m = JSON.parse(ev.data);
  if (m.id) { const p = pending.get(m.id); if (p) { pending.delete(m.id); m.error ? p.rej(new Error(JSON.stringify(m.error))) : p.res(m.result); } return; }
  if (m.method === "Runtime.consoleAPICalled") consoleMsgs.push((m.params.args || []).map((a) => String(a.value ?? a.description ?? "")).join(" ").slice(0, 200));
  if (m.method === "Log.entryAdded" && /wtui_genesis|Failed to resolve|取件/.test(m.params.entry.text || "")) consoleMsgs.push("[log] " + m.params.entry.text.slice(0, 200));
  if (m.method === "Network.loadingFailed") netFails.push(m.params.errorText + " " + (m.params.blockedReason || ""));
});
await new Promise((r) => ws.addEventListener("open", r));
await send("Runtime.enable"); await send("Log.enable"); await send("Network.enable"); await send("Page.enable");
await send("Page.navigate", { url: PAGE });

// 1) 等 boot 完成（log 面板出现「认领解析就绪」）
let ready = false;
for (let i = 0; i < 60; i++) {
  await sleep(500);
  const t = await evalJs("document.body ? document.body.innerText : ''");
  if (t && t.indexOf("认领解析就绪") >= 0) { ready = true; break; }
}
const bootText = await evalJs("document.body.innerText");
const consoleIsolated = await evalJs("self.crossOriginIsolated === true");
// 2) 浏览器侧直测：import map 键能否解析
let importType = null, importErr = null;
try { importType = await evalJs('import("wtui_genesis/std.js").then(m => typeof m.default)', true); } catch (e) { importErr = String(e.message).slice(0, 200); }
// 3) 点开 /dashboard.md（含 REGION 认领的真文档）
const clicked = await evalJs(`(() => { const els = [...document.querySelectorAll("div")].filter(d => d.children.length === 0 && d.textContent.trim() === "/dashboard.md"); if (!els.length) return false; els[0].click(); return true; })()`);
let docText = "";
for (let i = 0; i < 30; i++) {
  await sleep(400);
  docText = await evalJs("document.body.innerText");
  if (docText.indexOf("rev=") >= 0) break;
}
const logText = docText;
const checks = {
  bootReady: ready,
  crossOriginIsolated: consoleIsolated,
  importMapResolves_std: importType,
  importErr,
  dashboardClicked: clicked,
  docOpened: docText.indexOf("rev=") >= 0,
  logHasPickupFailure: logText.indexOf("取件失败") >= 0,
  logHasDeclError: logText.indexOf("decl ✗") >= 0,
  claimUnloadedMarker: logText.indexOf("件未加载") >= 0,
  regionRendered: docText.indexOf("◆") >= 0,
  pageHasWtuiChip: logText.indexOf("wtui_genesis") >= 0,
};
const shot = await send("Page.captureScreenshot", { format: "png" });
const fs = await import("node:fs");
fs.writeFileSync(process.argv[2] || "/tmp/cdp.png", Buffer.from(shot.data, "base64"));
console.log(JSON.stringify({ checks, consoleMsgs: consoleMsgs.slice(-12), netFails: netFails.slice(-5), logTail: logText.split("\n").filter((l) => /取件|decl|声明|认领解析|open |F 帧/.test(l)).slice(-12) }, null, 2));
ws.close();
