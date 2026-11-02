// test/pages-live.mjs —— exp_pages 两页的活体判据
import { installGuards, armWatchdog, guardTarget } from "./lib/watchdog.mjs";
installGuards("pages-live");
armWatchdog("pages-live", 91000);   /* = 91s；实测 45.2s（放宽到 280s 跑一次）⇒ ×2 */
//
// 口径：
//   · A 页 = 标量字段 + 一个「节点地址写」的 action（写回后整篇长度不得变）；
//   · B 页 = 列表字段（同名 tag 多行）+ V 层过滤 + 一个「+> 追加」的 action；
//   · 每段结束逐字还原，长度与开局一致（测试不留残渣 —— 这条是上一轮踩坑换来的）。
//
// 重要前提：本 wt 的 VFS **就是盘上目录** exp_pages/wt ⇒ 开局自愈直接写盘、再推一次即可。
const GW = process.env.GW || "http://127.0.0.1:8011";
const CDP = process.env.CDP || "http://127.0.0.1:9222";
guardTarget(CDP, "pages-live");   // 9222 = 业主 Chrome，默认拒绝
const BASE = process.env.BASE || "http://127.0.0.1:5178";
const WT = process.env.PAGES_WT || "c8Page01";
const path = await import("node:path");
const REPO = process.env.REPO || path.resolve(import.meta.dirname, "../../..");
const SHOT = process.env.SHOT_DIR || ".tmp/steps-gui";
// 目标 wt 未挂载 ⇒ ⊘ 跳过 + 退 0（--strict 退 1）：装载是可收敛的，缺件不是失败。
const { requireMountedWt } = await import("./wt-guard.mjs");
await requireMountedWt(GW, WT, "pages-live");
const NL = String.fromCharCode(10);
const fs = await import("node:fs");
const GWX = async (code) => {
  const r = await fetch(GW + "/exec", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ code, active_doc: "", wt_uid: WT }) });
  const j = await r.json();
  return String(j.result == null ? "" : j.result);
};
const R = [];
const chk = (cid, name, ok, detail) => R.push({ cid, name, ok: !!ok, detail: detail === undefined ? "" : String(detail).slice(0, 220) });

// 开局自愈：从磁盘把三份推入运行态（磁盘就是存储，推一次即同步）
const base = REPO + "/exp_pages/wt";
for (const [f, a] of [["/stats.md", "/stats.md"], ["/fields.md", "/fields.md"], ["/.mds/ui.md", "/.mds/ui.md"]]) {
  try { await GWX(JSON.stringify(fs.readFileSync(base + f, "utf8")) + " | /> " + a); } catch (e) { /* 自愈失败不掩盖断言 */ }
}
const A0 = String(await GWX("$ /stats.md"));
const B0 = String(await GWX("$ /fields.md"));

// ── 起页面 ──
const list = await (await fetch(CDP + "/json/list")).json();
for (const t of list) { if (t.type === "page") { try { await fetch(CDP + "/json/close/" + t.id); } catch (e) {} } }
const pg = await (await fetch(CDP + "/json/new?" + encodeURIComponent(BASE + "/?wt=" + WT), { method: "PUT" })).json();
const ws = new WebSocket(pg.webSocketDebuggerUrl);
let seq = 0; const pend = new Map();
const send = (m, pa) => new Promise((res) => { const i = ++seq; pend.set(i, res); try { ws.send(JSON.stringify({ id: i, method: m, params: pa })); } catch (e) { res({}); } });
ws.addEventListener("message", (ev) => { try { const m = JSON.parse(ev.data); if (m.id && pend.has(m.id)) { pend.get(m.id)(m.result || {}); pend.delete(m.id); } } catch (e) {} });
await new Promise((r) => ws.addEventListener("open", r));
await send("Runtime.enable"); await send("Page.enable");
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
// 按根门面（B：app 内容在影子树里）—— 探针一行不改。
const ROOT_SHIM = (expr) => "(() => { const g = window.document; const sr = (g.getElementById('app')||{}).shadowRoot; const document = sr ? { querySelector: (s) => sr.querySelector(s) || g.querySelector(s), querySelectorAll: (s) => { const a = [...sr.querySelectorAll(s)]; return a.length ? a : g.querySelectorAll(s); }, getElementById: (s) => sr.getElementById(s) || g.getElementById(s), body: { innerText: (g.body.innerText || '') + ' ' + (sr.textContent || ''), textContent: (g.body.textContent || '') + ' ' + (sr.textContent || '') }, documentElement: g.documentElement, createElement: (t) => g.createElement(t), addEventListener: g.addEventListener.bind(g), fonts: g.fonts, title: g.title } : g; return eval(" + JSON.stringify(expr) + "); })()";
const RAW_EV = (expr) => send("Runtime.evaluate", { expression: expr, returnByValue: true }).then((r) => (r.result ? r.result.value : "ERR"));
const ev = (x) => RAW_EV(ROOT_SHIM(x));
// 本文件已有 ROOT_SHIM ⇒ 复用（重复定义会语法错）
const evs = (x) => RAW_EV(ROOT_SHIM(x));
const shot = async (name) => { const s = await send("Page.captureScreenshot", { format: "png" }); try { fs.mkdirSync(SHOT, { recursive: true }); } catch (e) {} if (s && s.data) fs.writeFileSync(SHOT + "/" + name, Buffer.from(s.data, "base64")); };
const clickIn = async (sel) => {
  const b = await ev("JSON.stringify((() => { const el = document.querySelector(" + JSON.stringify(sel) + "); if (!el) return null; el.scrollIntoView(); const r = el.getBoundingClientRect(); return { x: Math.round(r.x + r.width / 2), y: Math.round(r.y + r.height / 2) }; })())");
  const p = JSON.parse(String(b) || "null");
  if (!p) return false;
  await send("Input.dispatchMouseEvent", { type: "mousePressed", x: p.x, y: p.y, button: "left", clickCount: 1 });
  await send("Input.dispatchMouseEvent", { type: "mouseReleased", x: p.x, y: p.y, button: "left", clickCount: 1 });
  return true;
};
const openDoc = async (path) => { await ev("([...((document.querySelector('#app-left')||{}).shadowRoot||document).querySelectorAll('.dv-ft-file')].find((e) => e.getAttribute('data-path') === " + JSON.stringify(path) + "))?.click()"); await wait(5500); };
const setInput = async (idx, val) => ev("(() => { const i = document.querySelectorAll('[data-page=\"fields\"] input')[" + idx + "]; if (!i) return; const d = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value'); d.set.call(i, " + JSON.stringify(val) + "); i.dispatchEvent(new Event('input', { bubbles: true })); })()");
const fTxt = async () => String(await ev("String((document.querySelector('[data-page=\"fields\"]')||{}).innerText||'')"));
const aTxt = async () => String(await ev("String((document.querySelector('[data-page=\"stats\"]')||{}).innerText||'')"));

for (let i = 0; i < 45; i += 1) { await wait(900); if (await ev("!!((document.querySelector('#app-left')||{}).shadowRoot||document).querySelector('.dv-ft-file')")) break; }
await wait(2500);
const tree = String(await ev("JSON.stringify([...((document.querySelector('#app-left')||{}).shadowRoot||document).querySelectorAll('.dv-ft-file')].map((e) => e.getAttribute('data-path')))"));
chk("P1", "文件树里两篇页面都在", tree.indexOf("/stats.md") >= 0 && tree.indexOf("/fields.md") >= 0, tree);

// ── A 页 ──
await openDoc("/stats.md");
const a1 = await aTxt();
chk("P2", "A 渲染：卡片在 + 含 128 + 记录为数字", (await ev("!!document.querySelector('[data-page=\"stats\"]')")) === true && a1.indexOf("128") >= 0 && /记录[:\s]*(\d+)/.test(a1), JSON.stringify(a1.slice(0, 80)));
chk("P3", "tailwind 任意值类名生效（卡片内有 rounded-* 后代）", (await ev("document.querySelectorAll('[data-page=\"stats\"] [class*=\"rounded-\"]').length")) >= 1);
const n0 = (a1.match(/记录[:\s]*(\d+)/) || [null, "?"])[1];
await clickIn('[data-page="stats"] button');
let n1 = null; for (let i = 0; i < 12; i += 1) { await wait(900); const m = (await aTxt()).match(/记录[:\s]*(\d+)/); if (m) n1 = m[1]; if (n1 && n1 !== n0) break; }
// 用**全路径形式**读节点，不依赖 active_doc（测试的 exec 没带它 ⇒ 直接写相对地址会 [V003] 缺省寻址）
const nodeA = String(await GWX("$ /stats.md::站点统计.面板._b:记录"));
const A1 = String(await GWX("$ /stats.md"));
chk("P4", "A 的 action：点按钮 ⇒ 节点值 +1", Number(nodeA) === Number(n0) + 1, JSON.stringify({ 前: n0, 节点: nodeA }));
chk("P5", "A 的渲染跟随（帧回来 ⇒ 重算）", n1 === nodeA, JSON.stringify({ 卡片: n1, 节点: nodeA }));
chk("P6", "A 是**外科式写**：整篇长度不变", A1.length === A0.length, A0.length + " → " + A1.length);
await shot("37-new-A.png");
await GWX(JSON.stringify(A0) + " | /> /stats.md");

// ── B 页 ──
await openDoc("/fields.md");
const b1 = await fTxt();
chk("P7", "B 渲染：卡片在 + 含 idx_email + 索引计数 2", (await ev("!!document.querySelector('[data-page=\"fields\"]')")) === true && b1.indexOf("idx_email") >= 0 && b1.indexOf("idx_created_at") >= 0, JSON.stringify(b1.slice(0, 90)));
await setInput(0, "created"); await wait(900);
const b2 = await fTxt();
chk("P8", "B 的 V 层过滤：输入 created ⇒ 只剩 idx_created_at", b2.indexOf("idx_created_at") >= 0 && b2.indexOf("idx_email") < 0, JSON.stringify(b2.slice(0, 90)));
await setInput(0, ""); await setInput(1, "idx_probe_v2"); await wait(700);
await clickIn('[data-page="fields"] button');
let added = false; for (let i = 0; i < 12; i += 1) { await wait(900); if ((await fTxt()).indexOf("idx_probe_v2") >= 0) { added = true; break; } }
const B1 = String(await GWX("$ /fields.md"));
chk("P9", "B 的 action：+> 追加一个 block（文档 + 卡片都多一行）", added && B1.indexOf("- 索引: idx_probe_v2") >= 0, JSON.stringify({ 卡片: added, 文档: B1.indexOf("idx_probe_v2") >= 0 }));
await shot("38-new-B.png");

// ── 还原 + 声明 ──
await GWX(JSON.stringify(B0) + " | /> /fields.md");
const A2 = String(await GWX("$ /stats.md"));
const B2 = String(await GWX("$ /fields.md"));
chk("P10", "逐字还原：两篇长度回到开局", A2.length === A0.length && B2.length === B0.length, JSON.stringify({ A: A0.length + "→" + A2.length, B: B0.length + "→" + B2.length }));
// /.mds 是折叠目录：先展开再点（不展开会点空、页面仍停在上篇 —— 上一轮踩过）
// DOM `.click()` 打不动文件树目录（实测：点 .mds 不展开）⇒ 用 CDP 真鼠标（同 check-doc-render 的结论）。
  {
    const pt = JSON.parse(String(await ev("(() => { const e = [...((document.querySelector('#app-left')||{}).shadowRoot||document).querySelectorAll('.dv-ft-dir')].find((d) => (d.innerText || '').indexOf('.mds') >= 0); if (!e) return 'null'; e.scrollIntoView(); const r = e.getBoundingClientRect(); return JSON.stringify({ x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2) }); })()")) || null);
    if (pt) {
      await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: pt.x, y: pt.y });
      await send("Input.dispatchMouseEvent", { type: "mousePressed", x: pt.x, y: pt.y, button: "left", clickCount: 1 });
      await send("Input.dispatchMouseEvent", { type: "mouseReleased", x: pt.x, y: pt.y, button: "left", clickCount: 1 });
    }
    await new Promise((r) => setTimeout(r, 500));
  }
await wait(1500);
await openDoc("/.mds/ui.md");
const d = String(await evs("String((((document.querySelector('.mdio-doc') || document.querySelector('.dv-app') || {}).innerText) || ''))"));   // 读**影子树里的**文档容器（`.dv-app` 是光 DOM 外壳 ⇒ innerText 恒空）
// 结论：`.dv-app` 是光 DOM 外壳，文档在影子树里 ⇒ 读 `.mdio-doc`
chk("P11", "声明文档自身渲染完整（STATS: 与 include: 都在）", d.indexOf("STATS:") >= 0 && d.indexOf("include:") >= 0, JSON.stringify(d.slice(0, 90)));

// P12（KaTeX 的 CSS mod 运行时三验）**暂时撤下**：VFS 里没有能渲染公式的文档，需要自带 fixture；
// 复验配方（`GWX` 写一篇含 $$…$$ 的临时文档 → `openDoc` 点开 → 断言三条 → 删除）。
try { await fetch(CDP + "/json/close/" + pg.id); } catch (e) {}
for (const r of R) console.log((r.ok ? " ✓ " : " ✗ ") + r.cid + " " + r.name + (r.detail ? "  → " + r.detail : ""));
const bad = R.filter((r) => !r.ok).length;
console.log(bad ? ("✗ pages-live 失败 " + bad + " 项") : ("✓ pages-live 全绿（" + R.length + " 项）"));
process.exitCode = bad ? 1 : 0;

// 收尾退出：老的测试脚本跑完不收 CDP socket ⇒ 不显式退出就会被自己的 deadline 判成卡死。
// 退出码沿用脚本自己设的 process.exitCode（没设 = 0）；失败路径若已 process.exit(1) 则不受影响。
process.exit(process.exitCode === undefined ? 0 : process.exitCode);
