// slot-live.mjs —— router 槽位的**活体**判据（真页面 · 自建无头 CDP）。
// 默认 9223（自建无头）；默认不连 9222（它可能是外部可见的浏览器实例）。
// 三段：① dev 面日志（boot: router appview=… · 诊断=0）
//       ② 带槽位的树：产品面出现被指名件的标记，且 app 内路由（页内段走 path）与 content（纸面）照旧
//       ③ 不带槽位的树：无标记、内容照旧（= 缺省回退）
// 跑法：node mdio_ui/demo/test/slot-live.mjs   （可覆盖 CDP / BASE / WT / WT_OTHER）
import { installGuards, makeEv, armWatchdog, guardTarget, withCallTimeout } from "./lib/watchdog.mjs";   // 超时 / 卡死检测 / 目标守卫（9222 = 业主 Chrome，默认拒绝）
const CDP = process.env.CDP || "http://127.0.0.1:9223";
guardTarget(CDP, "slot-live");   // 连 9222（业主 Chrome）直接拒绝
armWatchdog("slot-live", 10000)   /* = 10s */;   // 实测 max=2.5s（本机 2 跑）⇒ ×2=5.0s，取下限 10s ⇒ 10s
installGuards("slot-live");   // 未捕获异常 / 未处理拒绝 ⇒ 打印并 exit 4（不留「跑不完」）
const BASE = process.env.BASE || "http://127.0.0.1:5178";
const WT = process.env.WT || "czUR8zxu";
const WT_OTHER = process.env.WT_OTHER || "c8Page01";
let pass = 0, total = 0;
const chk = (n, ok, d) => { total += 1; if (ok) pass += 1; console.log((ok ? "  ok   " : "  FAIL ") + n + (d ? "  — " + String(d).slice(0, 220) : "")); };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const pageErrs = [];   // 页面异常账（模块级：多个 tab 的探针共用一本账）
const NL = "String.fromCharCode(10)";

async function open(url) {
  const tab = await (await fetch(CDP + "/json/new?" + encodeURIComponent(url), { method: "PUT" })).json();
  const w = new WebSocket(tab.webSocketDebuggerUrl);
  let seq = 0; const pend = new Map(); const errs = [];
  w.addEventListener("message", (e) => {
    try {
      const m = JSON.parse(e.data);
      if (m.id && pend.has(m.id)) { pend.get(m.id)(m.result || {}); pend.delete(m.id); }
      else if (m.method === "Runtime.exceptionThrown") errs.push(String((m.params.exceptionDetails.exception || {}).description || "").slice(0, 140));
    } catch (e2) { /* ignore */ }
  });
  const rawSend = (method, params) => new Promise((r) => { const i = ++seq; pend.set(i, r); w.send(JSON.stringify({ id: i, method, params })); });
const send = withCallTimeout("slot-live", rawSend);   // 单次 CDP 调用 20s 超时 ⇒ 挂起会抛异常而不是永远等
  await new Promise((r) => w.addEventListener("open", r));
  await send("Runtime.enable", {});
  const ev = makeEv(send, "slot-live", pageErrs);
  const waitFor = async (expr, tries) => { for (let k = 0; k < (tries || 40); k += 1) { if ((await ev(expr)) === true) return true; await sleep(700); } return false; };
  const close = async () => { await fetch(CDP + "/json/close/" + tab.id).catch(() => {}); };
  return { ev: ev, waitFor: waitFor, errs: errs, close: close };
}

// ① dev 面：日志里出现槽位解析行 + 无诊断
{
  const A = await open(BASE + "/?wt=" + WT);
  const up = await A.waitFor("!!document.querySelector('.dv-log-line')", 45);
  const logTxt = String(await A.ev("[...document.querySelectorAll('.dv-log-line')].map(l=>l.innerText).join(" + NL + ")") || "");
  chk("① dev 面在（日志面板有行）", up);
  chk("① 日志出现 boot: router appview=SCRIPT_APP", /boot: router appview=SCRIPT_APP/.test(logTxt), (logTxt.match(/boot: router appview=.*/) || [""])[0]);
  chk("① 声明装载无诊断（decl mods=… 诊断=0）", /boot: decl mods=\d+ match=\d+ 诊断=0/.test(logTxt), (logTxt.match(/boot: decl .*/) || [""])[0]);
  chk("① 无未捕获异常", A.errs.length === 0, JSON.stringify(A.errs.slice(0, 2)));
  // 「文档体真的画出来了」这一条用于拦截假绿：错误被 NodeView 的 ErrorBoundary 吃掉，
  // 所以 uncaught=0 与部分文本断言都可能过，而每个 h 节点其实都成了错误卡。
  const dom1 = JSON.parse(String(await A.ev("JSON.stringify({cards:document.querySelectorAll('.dv-eb').length,std:((document.getElementById('app')||{}).shadowRoot||document).querySelectorAll('.mdio-std').length,appMarker:!!((document.getElementById('app')||{}).shadowRoot||document).querySelector('[data-app=scripts]'),face:(document.querySelector('.mdio-root')||{getAttribute:()=>''}).getAttribute('data-face')})")) || "{}");
  chk("① 开发面里入口件也在（devview 包裹 appview：`#app` 在 workspace 里）", dom1.appMarker === true, JSON.stringify(dom1));
  chk("① dev 面无渲染错误卡（.dv-eb=0）", dom1.cards === 0, JSON.stringify(dom1));
  chk("① 文档体真的画了（.mdio-std ≥ 2）", dom1.std >= 2, JSON.stringify(dom1));
  await A.close();
}
// ② 带槽位的树：产品面标记 + 纸面 + 壳参数
{
  const url = BASE + "/edit/" + encodeURIComponent("剧本.md") + "?wt=" + WT + "&app=" + encodeURIComponent("/剧本2.md");
  const B = await open(url);
  const up = await B.waitFor("((document.getElementById('app')||{}).shadowRoot||document).querySelectorAll('.paper-sheet').length>0", 60);
  const st = JSON.parse(String(await B.ev("JSON.stringify({sheets:((document.getElementById('app')||{}).shadowRoot||document).querySelectorAll('.paper-sheet').length,marker:!!((document.getElementById('app')||{}).shadowRoot||document).querySelector('[data-app=scripts]'),std:((document.getElementById('app')||{}).shadowRoot||document).querySelectorAll('.mdio-std').length,cards:document.querySelectorAll('.dv-eb').length,search:location.search,path:location.pathname})")) || "{}");
  chk("② 槽位生效：被指名件的标记上屏", up && st.marker === true, JSON.stringify(st));
  chk("② content 通路未变：纸面照旧渲染", st.sheets > 0 && st.std > 0, JSON.stringify(st));
  chk("② 页内段在 path、壳段原样（?wt&app）", st.path.indexOf("/edit/") === 0 && st.search.indexOf("wt=" + WT) >= 0 && st.search.indexOf("app=") >= 0, st.path + st.search);
  chk("② 无渲染错误卡", st.cards === 0, JSON.stringify(st));
  chk("② 无未捕获异常", B.errs.length === 0, JSON.stringify(B.errs.slice(0, 2)));
  await B.close();
}
// ③ 不带槽位的树：无标记、内容照旧（缺省回退）
{
  const C = await open(BASE + "/?wt=" + WT_OTHER + "&app=" + encodeURIComponent("/stats.md"));
  // 注意：`Document.textContent` 规范上返回 **null**（我踩过），所以文档模式必须用 `body.innerText`。
  const up = await C.waitFor("(document.getElementById('app')&&document.getElementById('app').shadowRoot?document.getElementById('app').shadowRoot.textContent:((document.body&&document.body.innerText)||'')).indexOf('访问')>=0", 60);
  const st = JSON.parse(String(await C.ev("JSON.stringify({marker:!!((document.getElementById('app')||{}).shadowRoot||document).querySelector('[data-app=scripts]'),std:((document.getElementById('app')||{}).shadowRoot||document).querySelectorAll('.mdio-std').length,cards:document.querySelectorAll('.dv-eb').length,text:String(document.body.innerText).split(" + NL + ").join(' / ').slice(0,80)})")) || "{}");
  chk("③ 无槽位的树：无标记（缺省回退）", up && st.marker === false, JSON.stringify(st));
  chk("③ 无槽位的树：内容照旧渲染", st.std > 0, JSON.stringify(st));
  chk("③ 无渲染错误卡", st.cards === 0, JSON.stringify(st));
  chk("③ 无未捕获异常", C.errs.length === 0, JSON.stringify(C.errs.slice(0, 2)));
  await C.close();
}
chk("无页面异常（Runtime.evaluate 的 exceptionDetails）", pageErrs.length === 0, pageErrs.slice(0, 2).join(" | "));
console.log((pass === total ? "✓ 槽位活体判据全绿（" : "✗ 失败 ") + pass + "/" + total + (pass === total ? "）" : ""));
process.exit(pass === total ? 0 : 1);
