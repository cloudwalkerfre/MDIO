// app-live.mjs —— 「产品面 = 入口件就是整页的根」+「换篇闭环」的活体判据（真页面 · 自建无头 9223）。
// 模型：**面是两回事** ——
//   开发面（没有 ?app=）= 看 mods（md 里的 js / 作为数据源的 md）：外壳整块 + 明文文档；
//   产品面（有 ?app=）= 正经 app：**入口件就是整页的根，全视野归 appview**，外壳整块隐藏且不参与。
// 判据因此**只看产品面自己的信号**（左栏高亮 / URL / 文档体），不去借开发壳的 .dv-filetag。
// 跑法：node mdio_ui/demo/test/app-live.mjs   （可覆盖 CDP / BASE / WT / DOC / DOC2）
import { installGuards, makeEv, armWatchdog, guardTarget, withCallTimeout } from "./lib/watchdog.mjs";   // 超时 / 卡死检测 / 目标守卫（9222 = 业主 Chrome，默认拒绝）
const CDP = process.env.CDP || "http://127.0.0.1:9223";
guardTarget(CDP, "app-live");   // 连 9222（业主 Chrome）直接拒绝
armWatchdog("app-live", 15000)   /* = 15s */;   // 实测 max=7.3s（本机 2 跑）⇒ ×2=14.6s，取下限 10s ⇒ 15s
installGuards("app-live");   // 未捕获异常 / 未处理拒绝 ⇒ 打印并 exit 4（不留「跑不完」）
const BASE = process.env.BASE || "http://127.0.0.1:5178";
const WT = process.env.WT || "czUR8zxu";
const DOC = process.env.DOC || "/scripts/剧本.md";
const DOC2 = process.env.DOC2 || "/scripts/剧本M0备份.md";
const NAME2 = "剧本M0备份";
let pass = 0, total = 0;
const chk = (n, ok, d) => { total += 1; if (ok) pass += 1; console.log((ok ? "  ok   " : "  FAIL ") + n + (d ? "  — " + String(d).slice(0, 220) : "")); };
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
const send = withCallTimeout("app-live", rawSend);   // 单次 CDP 调用 20s 超时 ⇒ 挂起会抛异常而不是永远等
await send("Runtime.enable", {});
const pageErrs = [];   // 页面异常账（Runtime.evaluate 的 exceptionDetails）
const ev = makeEv(send, "app-live", pageErrs);
const waitFor = async (expr, tries) => { for (let k = 0; k < (tries || 40); k += 1) { if ((await ev(expr)) === true) return true; await sleep(600); } return false; };
const RECT = (s) => "(()=>{const e=document.querySelector('" + s + "');if(!e)return null;const r=e.getBoundingClientRect();return r.width>0&&r.height>0})()";
// B 路线（app 内容住进 `#app` 影子根）⇒ app 内容的查询必须**按根**：有影子根查影子根，没有则回退 document（两种模式共用一套判据）。
const SRC = "((document.getElementById('app')||{}).shadowRoot||document)";
const RQ = (s) => SRC + ".querySelector('" + s + "')";
const RQA = (s) => SRC + ".querySelectorAll('" + s + "')";
const STATE = "JSON.stringify({head:" + RECT('.dv-header') + ",panes:" + RECT('.dv-left') + ",foot:" + RECT('.dv-footer') + ",app:!!" + RQ('[data-app=scripts]') + ",items:[..." + RQA('[data-app-list] a') + "].map(a=>a.getAttribute('data-doc')),cur:(" + RQ('.app-scripts-item.on') + "||{}).textContent||'',href:(" + RQ('[data-app-list] a') + "||{getAttribute:()=>''}).getAttribute('href'),search:location.search,hash:location.hash,scRoot:" + RQA('.sc-root') + ".length,sheets:" + RQA('.sc-sheet') + ".length,cards:document.querySelectorAll('.dv-eb').length})";
const S = async () => JSON.parse(String(await ev(STATE)) || "{}");

// ① 产品面：入口件是整页的根；开发壳不参与
{
  const up = await waitFor(SRC + ".querySelectorAll('[data-app-list] a').length>=2", 45);
  const st = await S();
  chk("① 产品面：**header 常显** + workspace 三栏与 footer 隐藏 + 入口件在", up && st.head === true && st.panes === false && st.foot === false && st.app === true, JSON.stringify({ head: st.head, panes: st.panes, foot: st.foot, app: st.app }));
  chk("① 左栏清单 = 目录约定 /scripts/（两篇）", Array.isArray(st.items) && st.items.length === 2 && st.items.indexOf(DOC) >= 0 && st.items.indexOf(DOC2) >= 0, JSON.stringify(st.items));
  chk("① 当前篇高亮 + 壳段指向它", st.cur === "剧本" && decodeURIComponent(String(st.search)).indexOf(DOC) >= 0, JSON.stringify({ cur: st.cur, search: st.search }));
  chk("① 链接只写页内段（带壳段 + #/doc/）", String(st.href).indexOf("wt=" + WT) >= 0 && String(st.href).indexOf("#/doc/") > 0, String(st.href).slice(0, 120));
  chk("① 无渲染错误卡", st.cards === 0, JSON.stringify({ cards: st.cards }));
  chk("① 文档体真的画了（剧本按纸面排版 ⇒ .sc-root > 0）", st.scRoot > 0, JSON.stringify({ scRoot: st.scRoot }));
}
// ② 真点击换篇（产品面内完成，不需要开发者模式）
{
  const clickExpr = "(async () => { const R = " + SRC + "; const a = [...R.querySelectorAll('[data-app-list] a')].find(x => (x.getAttribute('data-doc')||'').indexOf('M0') >= 0); if (!a) return 'no-item'; a.click(); for (let i=0;i<48;i++){ await new Promise(r=>setTimeout(r,250)); const on=R.querySelector('.app-scripts-item.on'); if (on && on.textContent === '" + NAME2 + "') return 'opened'; } return 'timeout'; })()";
  const opened = await ev(clickExpr);
  await sleep(900);
  const st = await S();
  chk("② 真点击 ⇒ 换篇（左栏高亮换到目标篇）", opened === "opened" && st.cur === NAME2, JSON.stringify({ opened: opened, cur: st.cur }));
  chk("② 换篇后文档体仍在（同一套排版；两篇是同一稿 ⇒ 只认路径级变化）", st.scRoot > 0, JSON.stringify({ scRoot: st.scRoot }));
  chk("② URL 归一：篇名进 ?app=", decodeURIComponent(String(st.search)).indexOf(DOC2) >= 0 && decodeURIComponent(String(st.search)).indexOf("app=") >= 0, st.search);
  chk("② 页内段退回「页」（归一后 hash 清空）", String(st.hash) === "", JSON.stringify({ hash: st.hash }));
  chk("② 指针动作不改 wt / 不改面（header 仍在、三栏仍隐藏）", String(st.search).indexOf("wt=" + WT) >= 0 && st.head === true && st.panes === false, JSON.stringify({ search: st.search, head: st.head, panes: st.panes }));
  chk("② 无渲染错误卡", st.cards === 0, JSON.stringify({ cards: st.cards }));
}
// ③ history 同步（归一之后 back/forward 改的是 ?app=，必须有消费者）
{
  await ev("history.back()");
  await sleep(2600);
  const s1 = await S();
  chk("③ 后退：URL 与文档同步（不脱钩）", decodeURIComponent(String(s1.search)).indexOf(DOC) >= 0 && s1.cur === "剧本" && s1.scRoot > 0, JSON.stringify({ search: s1.search, cur: s1.cur, scRoot: s1.scRoot }));
  await ev("history.forward()");
  await sleep(2600);
  const s2 = await S();
  chk("③ 前进：回到目标篇", decodeURIComponent(String(s2.search)).indexOf(DOC2) >= 0 && s2.cur === NAME2 && s2.scRoot > 0, JSON.stringify({ search: s2.search, cur: s2.cur, scRoot: s2.scRoot }));
}
// ④ 左栏在换篇期间不重挂
{
  const items = Number(await ev(SRC + ".querySelectorAll('[data-app-list] a').length"));
  chk("④ 换篇后左栏清单仍是两篇（未重挂）", items === 2, JSON.stringify({ items: items }));
}
chk("⑤ 无未捕获异常", errs.length === 0, JSON.stringify(errs.slice(0, 2)));
chk("无页面异常（Runtime.evaluate 的 exceptionDetails）", pageErrs.length === 0, pageErrs.slice(0, 2).join(" | "));
console.log((pass === total ? "✓ 入口件活体判据全绿（" : "✗ 失败 ") + pass + "/" + total + (pass === total ? "）" : ""));
await fetch(CDP + "/json/close/" + tab.id).catch(() => {});
process.exit(pass === total ? 0 : 1);
