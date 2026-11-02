// test/sop-run.mjs —— 按 UI_TEST_SOP 在**真实（有界面）Chrome** 上实跑一遍，并逐步截图。
import { installGuards, armWatchdog, guardTarget } from "./lib/watchdog.mjs";
installGuards("sop-run");
armWatchdog("sop-run", 184000);   /* = 184s；实测 91.7s（放宽到 280s 跑一次）⇒ ×2 */
//
// 用法：
//   1) 先起正经 Chrome（不带头，带界面）：
//      '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' \
//        --remote-debugging-port=9222 --user-data-dir=/tmp/mdio-cdp --no-first-run about:blank
//   2) SHOT_DIR=.tmp/steps node test/sop-run.mjs
//
// 判据（SOP §3 Step6 三对齐 + §5.3 裁判）：
//   ① 写侧：console 结果含 mdio_success
//   ② 帧侧：log 栏出现 F 帧/P 帧、文档面 rev 递增
//   ③ 渲染侧：文档面文本出现新值
//   裁判：$ <addr> -pj ct 读回 == 文档面文本
// 另含 §3 截图级回归 L1/L2/L3 的机器可判部分。
const CDP = process.env.CDP || "http://127.0.0.1:9224";   // S0 要求**有界面** Chrome ⇒ 默认 9224（自建有头）；9223 是无头会红、9222 是业主 Chrome（guard 会拒）
guardTarget(CDP, "sop-run");   // 9222 = 业主 Chrome，默认拒绝
const BASE = process.env.BASE || "http://127.0.0.1:5178";
const GW = process.env.GW || "http://127.0.0.1:8011";
const SHOT_DIR = process.env.SHOT_DIR || ".tmp/steps";
const DOC = process.env.DOC || "/sop_probe.md";   // 用已存在的 scratch 文件（本网关无 __reset__ 钩子，/> 只能写已存在文件）
import { mkdirSync, writeFileSync } from "node:fs";
mkdirSync(SHOT_DIR, { recursive: true });

const R = [];
const chk = (id, name, ok, detail) => { R.push({ id, name, ok: !!ok, detail: detail === undefined ? "" : String(detail) }); };

// —— 0 版本自检：必须是"正经浏览器"（非 HeadlessChrome） ——
const ver = await (await fetch(CDP + "/json/version")).json();
const isHeadless = /Headless/i.test(ver["User-Agent"] || "");
chk("S0", "CDP 上是正经（有界面）Chrome", !isHeadless, ver["User-Agent"]);

// —— 1 seed（SOP Step1：先 seed 再 new_page） ——
const seedBody = { code: JSON.stringify("# T\\n\\n- k: v\\n") + " | /> " + DOC, active_doc: "", wt_uid: "genesis" };
const seedRes = await fetch(GW + "/exec", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(seedBody) }).then((r) => r.text()).catch((e) => "ERR " + e.message);
chk("S1", "seed 文档（写 # T + bullet 到已存在文件）", seedRes.indexOf("mdio_success") >= 0, seedRes.slice(0, 160));

// —— 2 close_old_tab + new_page（SOP Step2/3） ——
const list = await (await fetch(CDP + "/json/list")).json();
for (const t of list) { if (t.type === "page") { try { await fetch(CDP + "/json/close/" + t.id); } catch (e) {} } }
const pg = await (await fetch(CDP + "/json/new?" + encodeURIComponent(BASE + "/?wt=genesis"), { method: "PUT" })).json();
const ws = new WebSocket(pg.webSocketDebuggerUrl);
let id = 0; const pend = new Map(); const consoleLines = [];
const send = (m, pa) => new Promise((res) => { const i = ++id; pend.set(i, res); try { ws.send(JSON.stringify({ id: i, method: m, params: pa })); } catch (e) { res({}); } });
ws.addEventListener("message", (ev) => { try { const m = JSON.parse(ev.data);
  if (m.id && pend.has(m.id)) { pend.get(m.id)(m.result || {}); pend.delete(m.id); return; }
  if (m.method === "Runtime.consoleAPICalled") consoleLines.push(String(m.params.args.map((a) => String(a.value != null ? a.value : (a.description || ""))).join(" ")));
} catch (e) {} });
await new Promise((r) => ws.addEventListener("open", r));
await send("Runtime.enable"); await send("Page.enable");
// 按根门面（B：app 内容在影子树里）—— 探针一行不改。
const ROOT_SHIM = (expr) => "(() => { const g = window.document; const sr = (g.getElementById('app')||{}).shadowRoot; const document = sr ? { querySelector: (s) => sr.querySelector(s) || g.querySelector(s), querySelectorAll: (s) => { const a = [...sr.querySelectorAll(s)]; return a.length ? a : g.querySelectorAll(s); }, getElementById: (s) => sr.getElementById(s) || g.getElementById(s), body: { innerText: (g.body.innerText || '') + ' ' + (sr.textContent || ''), textContent: (g.body.textContent || '') + ' ' + (sr.textContent || '') }, documentElement: g.documentElement, createElement: (t) => g.createElement(t), addEventListener: g.addEventListener.bind(g), fonts: g.fonts, title: g.title } : g; return eval(" + JSON.stringify(expr) + "); })()";
const RAW_EV = (e, cap) => Promise.race([send("Runtime.evaluate", { expression: e, returnByValue: true }).then((r2) => (r2.result && r2.result.value)), new Promise((res) => setTimeout(() => res(null), cap || 15000))]);
const ev = (x) => RAW_EV(ROOT_SHIM(x));
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
const shot = async (name) => { const s = await send("Page.captureScreenshot", { format: "png" }); if (s && s.data) writeFileSync(SHOT_DIR + "/" + name, Buffer.from(s.data, "base64")); };
let boot = false;
for (let i = 0; i < 60 && !boot; i++) { await wait(1000); boot = !!(await ev("!!document.querySelector('.dv-wtbtn')")); }
chk("S2", "devview 启动（wt 按钮在）", boot, await ev("location.href"));
await shot("01-boot.png");

// —— 3 打开文档（SOP Step4） ——
  // DOM `.click()` 打不动文件树（现场实测：行数不变）⇒ 用 CDP 真鼠标点该行。
  {
    const __pt = JSON.parse(String(await ev("(() => { const e = [...document.querySelectorAll('.dv-ft-file')].find((x) => x.getAttribute('data-path') === " + JSON.stringify(DOC) + "); if (!e) return 'null'; e.scrollIntoView(); const r = e.getBoundingClientRect(); return JSON.stringify({ x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2) }); })()")) || null);
    if (__pt) {
      await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: __pt.x, y: __pt.y });
      await send("Input.dispatchMouseEvent", { type: "mousePressed", x: __pt.x, y: __pt.y, button: "left", clickCount: 1 });
      await send("Input.dispatchMouseEvent", { type: "mouseReleased", x: __pt.x, y: __pt.y, button: "left", clickCount: 1 });
    }
  }
let opened = false;
for (let i = 0; i < 40 && !opened; i++) { await wait(800); opened = !!(await ev("!!((document.getElementById('app')||{}).shadowRoot||document).querySelector('.mdio-std')")); }
chk("S3", "文档面渲染（.mdio-std 出现）", opened);
await shot("02-doc-open.png");

// —— 3b L1 截图级回归的机器可判部分：文件树每项高度 > 4 ——
const hs = await ev("JSON.stringify([...document.querySelectorAll('.dv-ft-file')].map((e) => Math.round(e.getBoundingClientRect().height)))");
let minH = 0; try { const arr = JSON.parse(hs || "[]"); minH = arr.length ? Math.min(...arr) : 0; } catch (e) {}
chk("L1", "文件树每项高度 > 4（不被压扁）", minH > 4, "minH=" + minH);

// —— 4 写入（SOP Step5：写通道只有 exec）+ 三对齐 ——
const logBefore = String(await ev("(document.querySelector('.dv-log')?.innerText || '').length"));
const wBody = { code: JSON.stringify("新值") + " | /> " + DOC + "::T._b", active_doc: "", wt_uid: "genesis" };
const wRes = await fetch(GW + "/exec", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(wBody) }).then((r) => r.text()).catch((e) => "ERR " + e.message);
chk("S4", "① 写侧：exec 受理（mdio_success）", wRes.indexOf("mdio_success") >= 0, wRes.slice(0, 120));
let shown = false;
for (let i = 0; i < 30 && !shown; i++) { await wait(800); shown = String(await ev("((() => { const el = document.getElementById('app'); const sr = el && el.shadowRoot; return ((sr ? sr.textContent : '') + ' ' + ((document.querySelector('.dv-app') || {}).innerText || '')); })())")).indexOf("新值") >= 0; }
chk("S5", "③ 渲染侧：文档面出现新值", shown);
const logText = String(await ev("(document.querySelector('.dv-log')?.innerText || '')"));
const logGrew = logText.length > Number(logBefore || 0);
chk("S6", "② 帧侧：log 增长（F/P 帧到达）", logGrew, "logLen " + logBefore + "→" + logText.length);
await shot("03-after-write.png");

// —— 5 裁判（SOP §5.3）：读回 == 渲染文本 ——
const rBody = { code: "$ " + DOC + "::T._b -pj ct", active_doc: "", wt_uid: "genesis" };
const rText = await fetch(GW + "/exec", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(rBody) }).then((r) => r.text()).catch(() => "");
const appText = String(await ev("((() => { const el = document.getElementById('app'); const sr = el && el.shadowRoot; return ((sr ? sr.textContent : '') + ' ' + ((document.querySelector('.dv-app') || {}).innerText || '')); })())"));
const judgeOk = rText.indexOf("新值") >= 0 && appText.indexOf("新值") >= 0;
chk("S7", "裁判：读回与文档面一致（都含新值）", judgeOk, "read=" + rText.trim().slice(0, 60));

// —— 6 Appview（产品面）L3：无 .dv-rev ——
await ev("location.href = " + JSON.stringify(BASE + "/?app=" + encodeURIComponent(DOC)));
let appOk = false;
for (let i = 0; i < 40 && !appOk; i++) { await wait(800); appOk = !!(await ev("!!((document.getElementById('app')||{}).shadowRoot||document).querySelector('.mdio-std')")); }   // Appview 的容器不是 .dv-app，用全局 .mdio-std
const revLine = await ev("!!document.querySelector('.dv-rev')");
chk("S8", "Appview 渲染", appOk);
chk("L3", "产品面无开发状态行（.dv-rev 不存在）", !revLine);
await shot("04-appview.png");

const failed = R.filter((r) => !r.ok);
for (const r of R) console.log((r.ok ? " ✓ " : " ✗ ") + r.id + " " + r.name + (r.ok ? "" : "  → " + r.detail));
console.log(failed.length ? "✗ SOP 实跑失败 " + failed.length + " 项" : "✓ SOP 实跑全绿（" + R.length + " 项）· 截图见 " + SHOT_DIR);
try { await fetch(CDP + "/json/close/" + pg.id); } catch (e) {}
process.exit(failed.length ? 1 : 0);