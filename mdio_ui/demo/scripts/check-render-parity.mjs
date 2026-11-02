// check-render-parity —— **孪生对照**活体判据：同一份内容，顶头加/不加 `# 标题` ⇒ 渲染必须一致。
import { installGuards, armWatchdog, guardTarget } from "../test/lib/watchdog.mjs";
installGuards("check-render-parity");
armWatchdog("check-render-parity", 120000);   /* = 120s；实测 60s（扫描 1 跑）⇒ ×2 */
//
// 为什么需要（业主报的现场）：无标题文档的正文整片不在 `.mdio-std` 排版作用域里 ⇒ 与有标题的
// 同一篇文档渲染不同（字号/行高/颜色/列表间距/引用边框全不一样）；根因是「文档体没带排版作用域」+「兜底没走 std」。
// 单看计数判据（li/p 数）**抓不到**这类问题——它们只验结构、不验排版 ⇒ 这条专门验**计算样式逐项相等**。
//
// 口径：不读 ui/不做全局探针（判官 R6a）；用**内容标记**等页面切到目标文档（不用固定 sleep —— 那是本仓
// 反复踩过的坑，见 check-doc-render 头注）；文档自建自清（失败也清）。
import { execSync } from 'node:child_process';
const BASE = process.env.BASE || 'http://127.0.0.1:5178';
const GW = process.env.MDIO_GATEWAY || 'http://127.0.0.1:8011';
const GW_PORT = process.env.MDIO_GW_UIDS === '1' ? '' : 'genesis';
const CDP = process.env.CDP || 'http://127.0.0.1:9222';
guardTarget(CDP, "check-render-parity");   // 9222 = 业主 Chrome，默认拒绝
const WT = process.env.WT || 'genesis';
const SHOT = process.env.SHOT_DIR || '.tmp/steps-gui/verify';
const A = '/parity_nohdr_probe.md', B = '/parity_hdr_probe.md';
const MARK_A = 'PARITYMARKA', MARK_B = 'PARITYMARKB';
const BODY = [
  '- user:',
  '  > PARITYMARKA PARITYMARKB body line',
  '  >',
  '  > - nested one',
  '  > - nested two',
].join(String.fromCharCode(10));
const gw = async (code) => {
  const r = await fetch(GW + '/exec', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ code, wt_uid: WT }) });
  return String(((await r.json()) || {}).result || '');
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const R = [];
const chk = (name, ok, detail) => R.push({ name, ok: !!ok, detail: detail === undefined ? '' : String(detail).slice(0, 240) });
// —— 自建两篇（ASCII 内容，避开管道对非 ASCII 的转义问题）
await gw('mk -f ' + A); await gw(JSON.stringify(BODY) + ' | /> ' + A);
await gw('mk -f ' + B); await gw(JSON.stringify('# PARITY HDR\n\n' + BODY) + ' | /> ' + B);
const list = await (await fetch(CDP + '/json/list')).json();
for (const t of list) if (t.type === 'page') { try { await fetch(CDP + '/json/close/' + t.id); } catch (e) {} }
const pg = await (await fetch(CDP + '/json/new?' + encodeURIComponent(BASE + '/?wt=' + WT), { method: 'PUT' })).json();
const ws = new WebSocket(pg.webSocketDebuggerUrl); let id = 0; const pend = new Map();
const send = (m, pa) => { const i = ++id; ws.send(JSON.stringify({ id: i, method: m, params: pa || {} })); return new Promise((res) => pend.set(i, res)); };
ws.addEventListener('message', (e) => { try { const m = JSON.parse(e.data); if (m.id && pend.has(m.id)) { pend.get(m.id)(m.result || {}); pend.delete(m.id); } } catch (err) {} });
await new Promise((r) => ws.addEventListener('open', r));
await send('Runtime.enable'); await send('Page.enable');
// （按根）：B 路线内容在 `#app` 的影子树里 —— 判内容的读要走影子根（缺的回退 document）。
const ROOT_SHIM = (expr) => "(() => { const g = window.document; const sr = (g.getElementById('app')||{}).shadowRoot; const document = sr ? new Proxy(sr, { get: (t, k) => { const v = (k in t) ? t[k] : g[k]; return typeof v === 'function' ? v.bind((k in t) ? t : g) : v; } }) : g; return (" + expr + "); })()";
const ev = (expr) => send('Runtime.evaluate', { expression: expr, returnByValue: true, awaitPromise: true }).then((r) => (r && r.result ? r.result.value : 'ERR'));
const evs = (x) => ev(ROOT_SHIM(x));   // 判**内容**用 evs（影子根优先）；判外壳（.dv-ft-file 等光 DOM）仍用 ev
const waitFor = async (expr, ms, step) => { const t0 = Date.now(); for (;;) { if (await ev(expr)) return true; if (Date.now() - t0 > ms) return false; await sleep(step || 400); } };
const STYLE_EXPR = "JSON.stringify((function(){ var d=document.querySelector('.mdio-doc'); if(!d) return 'no-doc'; var li=d.querySelector('li'), bq=d.querySelector('blockquote'), ks=d.querySelector('.md-bullet-key'); var s=getComputedStyle(d), sl=li?getComputedStyle(li):null, sb=bq?getComputedStyle(bq):null; return { docClass:d.className, docFS:s.fontSize, docLH:s.lineHeight, docColor:s.color, liFS:sl?sl.fontSize:null, liMT:sl?sl.marginTop:null, bqBorder:sb?sb.borderLeftWidth:null, bqPad:sb?sb.paddingLeft:null, keys:d.querySelectorAll('.md-bullet-key').length, li:d.querySelectorAll('li').length, std:d.className.indexOf('mdio-std')>=0 }; })())";
const openAndRead = async (path, marker) => {
  await ev('(function(){ var r=[...((document.querySelector("#app-left")||{}).shadowRoot||document).querySelectorAll(".dv-ft-file")].find(function(e){return e.getAttribute("data-path")===' + JSON.stringify(path) + ';}); if(r) r.click(); return !!r; })()');
  const landed = await waitFor('(function(){var g=window.document; var sr=(g.getElementById("app")||{}).shadowRoot||g; var d=sr.querySelector(".mdio-doc"); return !!d && d.innerText.indexOf(' + JSON.stringify(marker) + ')>=0;})()', 25000, 400);   // 内容在影子树里 ⇒ 内容探针按根
  if (!landed) return { landed: false };
  await sleep(600);
  const __raw = String((await evs(STYLE_EXPR)) || '{}');   // 解析失败时把原值带出来（诊断）
  try { return JSON.parse(__raw); } catch (e) { return { parse: String(e.message).slice(0, 50), raw: __raw.slice(0, 90) }; }
};
let ra = null, rb = null;
try {
  const okTree = await waitFor('!!((document.querySelector("#app-left")||{}).shadowRoot||document).querySelector(".dv-ft-file")', 60000, 900);
  chk('页面就绪（文件树出现）', okTree, okTree ? '' : '60s 内未见 .dv-ft-file');
  ra = await openAndRead(A, MARK_A);
  rb = await openAndRead(B, MARK_B);
  chk('两篇都打开（靠内容标记判，不用固定 sleep）', ra && rb && ra.landed !== false && rb.landed !== false, JSON.stringify({ a: ra && ra.landed, b: rb && rb.landed }));
  const KEYS = ['docFS', 'docLH', 'docColor', 'liFS', 'liMT', 'bqBorder', 'bqPad', 'keys', 'li'];
  const diff = KEYS.filter((k) => ra && rb && ra[k] !== rb[k]).map((k) => k + ': ' + JSON.stringify(ra[k]) + ' vs ' + JSON.stringify(rb[k]));
  chk('孪生计算样式逐项相等（加/不加标题）', diff.length === 0, diff.join(' ｜ '));
  chk('两篇都在 .mdio-std 排版作用域内', !!(ra && ra.std) && !!(rb && rb.std), JSON.stringify({ a: ra && ra.docClass, b: rb && rb.docClass }));
  for (const [nm, p] of [['parity-nohdr.png', A], ['parity-hdr.png', B]]) {
    const s = await send('Page.captureScreenshot', { format: 'png' });
    try { const fs2 = await import('node:fs'); fs2.mkdirSync(SHOT, { recursive: true }); if (s && s.data) fs2.writeFileSync(SHOT + '/' + nm, Buffer.from(s.data, 'base64')); } catch (e) {}
  }
} catch (e) { chk('探针执行', false, String((e && e.message) || e).slice(0, 200)); }
// —— 自清（失败也清）
try { await gw('rm "' + A + '"'); await gw('rm "' + B + '"'); } catch (e) {}
const failed = R.filter((x) => !x.ok);
for (const x of R) console.log((x.ok ? ' ✓ ' : ' ✗ ') + x.name + (x.ok ? '' : '  → ' + x.detail));
console.log(failed.length ? '✗ check-render-parity 失败 ' + failed.length + ' 项' : '✓ check-render-parity 全绿（' + R.length + ' 项）');
try { await fetch(CDP + '/json/close/' + pg.id); } catch (e) {}
process.exit(failed.length ? 1 : 0);