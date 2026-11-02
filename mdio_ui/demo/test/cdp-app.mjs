import { writeFileSync } from "node:fs";
const CDP = "http://127.0.0.1:9222";
const GW = "http://127.0.0.1:8011";
const page = await (await fetch(CDP + "/json/new?" + encodeURIComponent("http://127.0.0.1:5178/?app=%2Fdashboard.md"), { method: "PUT" })).json();
const ws = new WebSocket(page.webSocketDebuggerUrl);
let id = 0; const pend = new Map();
const send = (method, params) => new Promise((res, rej) => { const i = ++id; pend.set(i, { res, rej }); ws.send(JSON.stringify({ id: i, method, params })); });
const found = [];
ws.addEventListener("message", (ev) => {
  try {
    const m = JSON.parse(ev.data);
    if (m.id && pend.has(m.id)) { const p = pend.get(m.id); pend.delete(m.id); m.error ? p.rej(new Error(m.error.message)) : p.res(m.result); return; }
    if (m.method === "Runtime.exceptionThrown") found.push({ kind: "exc", detail: JSON.stringify(m.params.exceptionDetails).slice(0, 700) });
    if (m.method === "Runtime.consoleAPICalled" && m.params.type === "error") found.push({ kind: "err", args: m.params.args.map((a) => (a.value != null ? String(a.value) : String(a.description || ""))).slice(0, 3) });
  } catch (e) { /* ignore */ }
});
await new Promise((r) => ws.addEventListener("open", r));
await send("Runtime.enable"); await send("Page.enable");
const body = async () => (await send("Runtime.evaluate", { expression: "document.body.innerText", returnByValue: true })).result.value;
let rendered = false; let bootText = "";
for (let i = 0; i < 40; i++) {
  await new Promise((r) => setTimeout(r, 1000));
  const t = await body();
  if (t && t.indexOf("DASHBOARD") >= 0) { rendered = true; bootText = t.slice(0, 200); break; }
  if (i === 39) bootText = String(t || "").slice(0, 300);
}
// 读第一个引用 bullet 的真 content（-pj ct，无标记）→ 写探针 → 等自动更新 → 还原
const exec = async (code) => {
  const r = await fetch(GW + "/exec", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ code, active_doc: "", wt_uid: "genesis" }) });
  const j = await r.json();
  return String(j.result == null ? "" : j.result);
};
const SELF = "/dashboard.md::DASHBOARD.引用._b";
const NEEDLE = "APPVIEWPROBE77";
const orig = (await exec("$ " + SELF + " -pj ct")).trim();
await exec(JSON.stringify(NEEDLE) + " | /> " + SELF);
let updated = false;
for (let i = 0; i < 12; i++) {
  await new Promise((r) => setTimeout(r, 800));
  const t = await body();
  if (t && t.indexOf(NEEDLE) >= 0) { updated = true; break; }
}
await exec(JSON.stringify(orig) + " | /> " + SELF);
writeFileSync("/tmp/cdp-app.json", JSON.stringify({ rendered, updated, orig: orig.slice(0, 60), bootText, found: found.slice(0, 5) }, null, 1));
