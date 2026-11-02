// face-live.mjs —— 「形态（开发面/产品面）与指针（wt/文档）解耦」的活体判据（真页面 · 自建无头 9223）。
// 缘起（业主实测）：在 genesis 关掉开发者模式，再切 wt 到 exp_scripts ⇒ 开发者模式**跳回来了**。
// 根因：形态由 URL 的 ?app= 推导，而两个 URL 写点各自重写整条壳查询 —— 换树无条件清 APP_DOC、
//       开关形态时又只写 /?app=…（把 ?wt= 丢了）。修法：形态是壳状态；换树只改指针；URL 由一处组合。
// 跑法：node mdio_ui/demo/test/face-live.mjs   （可覆盖 CDP / BASE）
import { installGuards, makeEv, armWatchdog, guardTarget, withCallTimeout } from "./lib/watchdog.mjs";   // 超时 / 卡死检测 / 目标守卫（9222 = 业主 Chrome，默认拒绝）
const CDP = process.env.CDP || "http://127.0.0.1:9223";
guardTarget(CDP, "face-live");   // 连 9222（业主 Chrome）直接拒绝
armWatchdog("face-live", 28000)   /* = 28s */;   // 实测 max=13.9s（本机 2 跑）⇒ ×2=27.8s，取下限 10s ⇒ 28s
installGuards("face-live");   // 未捕获异常 / 未处理拒绝 ⇒ 打印并 exit 4（不留「跑不完」）
const BASE = process.env.BASE || "http://127.0.0.1:5178";
let pass = 0, total = 0;
const chk = (n, ok, d) => { total += 1; if (ok) pass += 1; console.log((ok ? "  ok   " : "  FAIL ") + n + (d ? "  — " + String(d).slice(0, 200) : "")); };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const tab = await (await fetch(CDP + "/json/new?" + encodeURIComponent(BASE + "/?wt=genesis"), { method: "PUT" })).json();
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
const send = withCallTimeout("face-live", rawSend);   // 单次 CDP 调用 20s 超时 ⇒ 挂起会抛异常而不是永远等
await send("Runtime.enable", {});
const pageErrs = [];   // 页面异常账（Runtime.evaluate 的 exceptionDetails）
const ev = makeEv(send, "face-live", pageErrs);
const waitFor = async (expr, tries) => { for (let k = 0; k < (tries || 40); k += 1) { if ((await ev(expr)) === true) return true; await sleep(600); } return false; };
const S = async () => JSON.parse(String(await ev("JSON.stringify({search:location.search,product:(document.querySelector('.dv-root')||{className:''}).className.indexOf('is-product')>=0,side:(()=>{const e=document.querySelector('.dv-left');return !!e && getComputedStyle(e).display!=='none'})(),wt:(()=>{const b=document.querySelector('.dv-wtbtn');return b?b.innerText.split(String.fromCharCode(10)).join(' ').trim():''})()})")) || "{}");
const toggleDev = async () => { await ev("(()=>{const b=[...document.querySelectorAll('.dv-toggle')].find(x=>x.innerText.indexOf('开发者模式')>=0); if(b) b.click(); return 1})()"); await sleep(1200); };
const pickWt = async (uid) => {
  await ev("(()=>{const b=document.querySelector('.dv-wtbtn'); if(b) b.click(); return 1})()"); await sleep(700);
  await ev("(()=>{const it=[...document.querySelectorAll('.dv-wtitem')].find(e=>((e.querySelector('.uid')||{}).textContent||'').indexOf('" + uid + "')>=0); if(it) it.click(); return it?1:0})()");
  await sleep(4000);
};

await waitFor("!!document.querySelector('.dv-root')", 45); await sleep(1200);
let s = await S();
chk("① 起点：genesis 开发面", s.product === false && s.side === true && s.search.indexOf("wt=genesis") >= 0, JSON.stringify(s));

await toggleDev();
s = await S();
chk("② 关开发者模式：形态=产品面", s.product === true && s.side === false, JSON.stringify(s));
chk("② 关形态**不许丢 wt**（旧行为丢 wt=genesis）", s.search.indexOf("wt=genesis") >= 0, s.search);
chk("② 产品面 URL 带 app=", s.search.indexOf("app=") >= 0, s.search);

await pickWt("czUR8zxu");
s = await S();
chk("③ 产品面里切树：**形态不跳**（仍是产品面、三栏仍隐藏）", s.product === true && s.side === false, JSON.stringify(s));
chk("③ 切树改的是指针：wt=czUR8zxu 且 app= 还在", s.search.indexOf("wt=czUR8zxu") >= 0 && s.search.indexOf("app=") >= 0, s.search);

await toggleDev();
s = await S();
chk("④ 在 exp_scripts 开回开发者模式：形态=开发面", s.product === false && s.side === true, JSON.stringify(s));
chk("④ 开形态同样不许丢 wt", s.search.indexOf("wt=czUR8zxu") >= 0 && s.search.indexOf("app=") < 0, s.search);

await pickWt("genesis");
s = await S();
chk("⑤ 开发面里切回 genesis：形态仍是开发面", s.product === false && s.side === true && s.search.indexOf("wt=genesis") >= 0, JSON.stringify(s));
chk("⑤ 无未捕获异常", errs.length === 0, JSON.stringify(errs.slice(0, 2)));
chk("无页面异常（Runtime.evaluate 的 exceptionDetails）", pageErrs.length === 0, pageErrs.slice(0, 2).join(" | "));
console.log((pass === total ? "✓ 形态/指针解耦判据全绿（" : "✗ 失败 ") + pass + "/" + total + (pass === total ? "）" : ""));
await fetch(CDP + "/json/close/" + tab.id).catch(() => {});
process.exit(pass === total ? 0 : 1);
