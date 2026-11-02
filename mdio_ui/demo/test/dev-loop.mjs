// dev-loop —— 判据：dev 期**改树内 mod 源码 ⇒ 宿主自动重编 ⇒ 页面整页重载**。
//
// 为什么是这条判据：
//   · 定稿"重建 + 整页重载"（模块级热替换不可达：模块表无淘汰 API + import map 不可改，规范级）；
//   · 实测闭环 123ms（小 mod）/ 2.4s（真 mod）—— 机制可行，但**一直没接进宿主** ⇒ 本判据把它钉住。
//   · 观测面：`/__mdio/dev-seq`（重建序号）+ 页面 `performance.timeOrigin`（是否真的重载了）。
//
// 收尾：探针注释会被还原；失败也还原（finally）。
import fs from "node:fs";
import path from "node:path";

const CDP = "http://127.0.0.1:9222", BASE = "http://127.0.0.1:5178";
const REPO = path.resolve(import.meta.dirname, "../../..");       // demo/test → demo → mdio_ui → 仓库根
const SRC = path.join(REPO, "worktrees/genesis/ui/region/src/region.js");
const R = [];
const chk = (cid, name, ok, detail) => R.push({ cid, name, ok: !!ok, detail: detail === undefined ? "" : String(detail).slice(0, 200) });
const seqOf = async () => { const r = await fetch(BASE + "/__mdio/dev-seq", { cache: "no-store" }); return (await r.json()).seq; };

const list = await (await fetch(CDP + "/json/list")).json();
for (const t of list) { if (t.type === "page") { try { await fetch(CDP + "/json/close/" + t.id); } catch (e) { /* ignore */ } } }
const pg = await (await fetch(CDP + "/json/new?" + encodeURIComponent(BASE + "/?wt=genesis"), { method: "PUT" })).json();
const ws = new WebSocket(pg.webSocketDebuggerUrl);
let id = 0; const pend = new Map();
const send = (m, pa) => new Promise((res) => { const i = ++id; pend.set(i, res); ws.send(JSON.stringify({ id: i, method: m, params: pa })); });
ws.addEventListener("message", (ev) => { const m = JSON.parse(ev.data); if (m.id && pend.has(m.id)) { pend.get(m.id)(m); pend.delete(m.id); } });
await new Promise((r) => ws.addEventListener("open", r));
await send("Runtime.enable");
const ev = async (e) => { const r = await send("Runtime.evaluate", { expression: e, returnByValue: true }); return r.result && r.result.result ? r.result.result.value : null; };
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
const until = async (fn, tries) => { for (let i = 0; i < tries; i += 1) { if (await fn()) return true; await wait(500); } return false; };

const original = fs.readFileSync(SRC, "utf8");
const MARK = "// MDIO_DEVLOOP_PROBE " + Date.now();
let seq0 = -1, t0 = null;
try {
  const ready = await until(async () => (await ev("!!document.querySelector('.dv-app')")), 60);
  chk("L0", "页面就绪（devview 挂上）", ready);
  await wait(1500);
  seq0 = await seqOf();
  t0 = await ev("performance.timeOrigin");
  chk("L1", "重建序号端点可用（含正在监视的包数）", seq0 >= 0, "seq=" + seq0);

  fs.writeFileSync(SRC, original + "\n" + MARK + "\n");
  const bumped = await until(async () => (await seqOf()) > seq0, 40);      // ≈20s 上限
  chk("L2", "改源码 ⇒ 宿主自动重编（序号自增）", bumped, "seq " + seq0 + " → " + (await seqOf()));
  const reloaded = await until(async () => (await ev("performance.timeOrigin")) !== t0, 20);
  chk("L3", "重编后 ⇒ 页面整页重载（timeOrigin 变）", reloaded);
  const alive = await until(async () => (await ev("!!document.querySelector('.dv-app')")), 30);
  chk("L4", "重载后页面仍可用（devview 回来）", alive);
} finally {
  fs.writeFileSync(SRC, original);                                          // 还原源码（失败也还原）
  const back = await until(async () => (await seqOf()) > seq0 + 1, 40);
  chk("L5", "源码还原 ⇒ 再重编一次（闭环可逆）", back, "seq=" + (await seqOf()));
}

const code = await (await fetch(BASE + "/?wt=genesis")).status;
chk("L6", "收尾页面 200", code === 200, "HTTP " + code);

for (const r of R) console.log((r.ok ? " ✓ " : " ✗ ") + r.cid + " " + r.name + (r.ok ? "" : "  → " + r.detail));
const bad = R.filter((r) => !r.ok).length;
console.log(bad === 0 ? "✓ dev-loop 全绿（" + R.length + " 项）" : "✗ dev-loop 失败 " + bad + " 项");
process.exit(bad === 0 ? 0 : 1);