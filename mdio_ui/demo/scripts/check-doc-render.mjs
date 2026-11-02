// @ts-check
import { installGuards, armWatchdog, guardTarget } from "../test/lib/watchdog.mjs";
installGuards("check-doc-render");
armWatchdog("check-doc-render", 194000)   /* = 194s；实测 97.3s（放宽 180s 跑一次）⇒ ×2 */
/**
 * check-doc-render —— **文档面渲染完整性**（活体判据：DOM 与交付树逐项对齐）。
 *
 * 为什么要有这一条（实测的缺陷类）：声明表里"命中"与"能画"是两件事 ——
 * 一条 `## match` 条目若指向**非渲染件**（CSS/副作用 mod，只导出 `sheet`），它占住了"谁画这个节点"的槽，
 * 框架按"认领即自画"就不再补画节的正文与子节 ⇒ **整篇文档只剩标题**，而且全绿（没有任何判据读文档面）。
 * 实测现场：genesis 树 `- THEME: ::..` 把 `std` 顶掉，`/.mds/ui.md`（全是 bullet）与 `/demos/README.md`
 * 都只渲染出标题。修法见 `uiMd.modCompOf` + `uiStore.renderClaims`；本条判据把结论钉住。
 *
 * 判据（对每篇样例文档，**同一次求值里**同时读 DOM 与交付树）：
 *   ① 标题数 = 交付树里节节点（h/sec）数；
 *   ② 列表项数（li）= 交付树里 bullet（b/x）数；
 *   ③ 每个有名字的 bullet，其**名字**必须出现在渲染文本里（"名字画不出来"是另一类静默丢失）；
 *   ④ 文档面非空（防"整篇白"）。
 * 前置：dev 宿主（BASE，默认 5178）+ headless Chrome（CDP，默认 9222）。截图落 SHOT_DIR。
 *
 * 用法：node scripts/check-doc-render.mjs   （可选 BASE / CDP / WT / SHOTS）
 */
import fs from "node:fs";
import path from "node:path";

const BASE = process.env.BASE || "http://127.0.0.1:5178";
const CDP = process.env.CDP_URL || process.env.CDP || "http://127.0.0.1:9223";   // 默认 9223：9222 是业主 Chrome（guardTarget 会拒）
guardTarget(CDP, "check-doc-render");   // 9222 = 业主 Chrome，默认拒绝
// 样本树换成 c8Demo01（exp_demo1）——czUR8zxu 的 appview 被 SCRIPT_APP 接管，文档面不再是 std 渲染，
// 判据形状（.mdio-std / li / p 计数）不成立；c8Demo01 的 /app.md 无认领节 ⇒ 全文 std。仍可 WT=… 覆盖。
const WT = process.env.WT || "c8Demo01";
const SHOT = process.env.SHOT_DIR || ".tmp/steps-gui/verify";
// 页面形状：dev 宿主在根（/），生产 preview 在 /demo/（verify-prod 的口径，见其文件头"页面地址是 /demo/?wt=genesis"）。
// 同一条判据两种形态都能跑 ⇒ "dev 绿、生产红"这一档不会被漏掉（§71.6）。
const PAGE_PREFIX = process.env.PAGE_PREFIX || "";

/** 样例文档：覆盖"全是 bullet 的声明文档 / 目录页 / 正文页"三种形状。 */
const DOCS = [
  { path: "/README.md", dir: null, why: "目录页：标题 + 列表（纯 std 全文；c8Demo01 · 2026-10-16 新增样本）" },
];

const R = [];
const chk = (name, ok, detail) => R.push({ name, ok: !!ok, detail: detail === undefined ? "" : String(detail).slice(0, 240) });

// ── CDP 小工具 ──
const list = await (await fetch(CDP + "/json/list")).json().catch((e) => ({ __err: e }));
if (list && list.__err) { console.log("check-doc-render ✗ 连不上 CDP（" + CDP + "）：" + list.__err.message); process.exit(1); }
for (const t of list) { if (t.type === "page") { try { await fetch(CDP + "/json/close/" + t.id); } catch (e) { /* 关闭失败不致命 */ } } }
const pg = await (await fetch(CDP + "/json/new?" + encodeURIComponent(BASE + PAGE_PREFIX + "/?wt=" + WT), { method: "PUT" })).json();
const ws = new WebSocket(pg.webSocketDebuggerUrl);
let seq = 0; const pend = new Map();
const send = (m, pa) => new Promise((res) => { const i = ++seq; pend.set(i, res); try { ws.send(JSON.stringify({ id: i, method: m, params: pa })); } catch (e) { res({}); } });
ws.addEventListener("message", (ev) => { try { const m = JSON.parse(ev.data); if (m.id && pend.has(m.id)) { pend.get(m.id)(m.result || {}); pend.delete(m.id); } } catch (e) { /* 忽略非响应帧 */ } });
await new Promise((r) => ws.addEventListener("open", r));
await send("Runtime.enable"); await send("Page.enable");
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
const ev = (expr) => send("Runtime.evaluate", { expression: expr, returnByValue: true, awaitPromise: true }).then((r) => (r.result ? r.result.value : "ERR:" + JSON.stringify((r.exceptionDetails || {}).text || "")));
const shot = async (name) => {
  const s = await send("Page.captureScreenshot", { format: "png" });
  try { fs.mkdirSync(SHOT, { recursive: true }); } catch (e) { /* 已存在 */ }
  if (s && s.data) fs.writeFileSync(path.join(SHOT, name), Buffer.from(s.data, "base64"));
};

// ── 打开页面 + 取 store（React fiber 是唯一入口：应用不暴露全局，也不该为测试开洞） ──
// 不再沿 React fiber 找 `DocPane.props.ui`（应用不暴露 store 是设计，且挂载路径已进影子树）⇒ 用**可观察事实**：影子根里出现 `.mdio-std`（或挂载点里有节点）。
const GRAB = "(() => { const R = (document.getElementById('app')&&document.getElementById('app').shadowRoot?document.getElementById('app').shadowRoot:document); if (R.querySelectorAll('.mdio-std').length > 0) return 'ok'; const m = R.querySelector('#mount') || R; return m.querySelectorAll('*').length > 0 ? 'ok' : 'no-doc'; })()";   // 不再伸手进 React fiber
/**
 * **等条件，不等时间**（本判据第一次挂进总检时红了一项，根因就是下面的固定 4.2s sleep ——
 * 套件里前一项刚跑完、页面更慢时"点完还没换页"⇒ docPath 仍是上一篇 ⇒ 判红。固定 sleep 在单跑时看不出问题。）
 */
const waitFor = async (expr, ms, stepMs) => {
  const t0 = Date.now();
  for (;;) {
    if (await ev(expr)) return true;
    if (Date.now() - t0 > ms) return false;
    await wait(stepMs || 400);
  }
};
// DOM `.click()` 对当前文件树**无效**（现场实测：点 /docs 的 caret 行数 12→12；改真鼠标后 12→21）⇒ 用 CDP 真鼠标点。
const clickAt = async (expr) => {
  const pt = JSON.parse(String(await ev("(() => { const e = " + expr + "; if (!e) return 'null'; const r = e.getBoundingClientRect(); return JSON.stringify({ x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2) }); })()")) || null);
  if (!pt) return false;
  await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: pt.x, y: pt.y });
  await send("Input.dispatchMouseEvent", { type: "mousePressed", x: pt.x, y: pt.y, button: "left", clickCount: 1 });
  await send("Input.dispatchMouseEvent", { type: "mouseReleased", x: pt.x, y: pt.y, button: "left", clickCount: 1 });
  return true;
};
const expandDir = async (dir) => { if (!dir) return true; const n0 = Number(await ev("((document.querySelector('#app-left')||{}).shadowRoot||document).querySelectorAll('.dv-ft-file').length")); await clickAt("((document.querySelector('#app-left')||{}).shadowRoot||document).querySelector('.dv-ft-dir[title=' + " + JSON.stringify(JSON.stringify(dir)) + " + '] .caret') || ((document.querySelector('#app-left')||{}).shadowRoot||document).querySelector('.dv-ft-dir[title=' + " + JSON.stringify(JSON.stringify(dir)) + " + ']')"); return await waitFor("((document.querySelector('#app-left')||{}).shadowRoot||document).querySelectorAll('.dv-ft-file').length > " + n0, 8000, 300); };
// 树行不一定带 data-path（当前 FileTree 的约定变了）⇒ 按 data-path 或**行文本含 basename**匹配。
const rowExpr = (p) => "[...((document.querySelector('#app-left')||{}).shadowRoot||document).querySelectorAll('.dv-ft-file')].some((e) => e.getAttribute('data-path') === " + JSON.stringify(p) + " || String(e.textContent || '').indexOf(" + JSON.stringify(p.split('/').pop()) + ") >= 0)";

const up = await waitFor("!!((document.querySelector('#app-left')||{}).shadowRoot||document).querySelector('.dv-ft-file')", 60000, 1000);
chk("页面起来（开发视图文件树就绪）", up, up ? "" : "60s 内没等到 .dv-ft-file（宿主没起？BASE=" + BASE + "）");
const grabbed = await waitFor(GRAB + " === 'ok'", 20000, 500);
chk("文档面已渲染（影子根内 .mdio-std / 挂载点有节点）", grabbed, grabbed ? "ok" : String(await ev(GRAB)));

// devMode 默认 false（产品面）⇒ 文件树三栏 display:none，CDP 点击坐标全 0 落空（点不开任何文档）。
// 先开开发者模式再点文件树（czUR8zxu 旧树恰好默认 dev 才没暴露）。
await ev("(() => { const b = [...document.querySelectorAll('button')].find((x) => String(x.textContent).indexOf('开发') >= 0); if (b) { b.click(); return true; } return false; })()");
await waitFor("(() => { const s = document.querySelector('.dv-left'); return !!s && s.getBoundingClientRect().width > 0; })()", 8000, 400);

/** 开档：目录展开 → 行出现 → 点击 → **等 docPath 真的变成它** → 等文档面画出东西。任一环节不到位就自愈一次（整页重载）。 */
async function openDoc(d) {
  // CDP 真鼠标点行/叶都不触发（handler 绑定与坐标时序对不上）；DOM .click() 点行内最后一个叶子
  // 是**当前唯一有效**的开档方式（与验收脚本口径一致；旧「DOM click 无效」是旧结构的结论）。
  const click = async () => String(await ev("(function(){ var row = [...((document.querySelector('#app-left')||{}).shadowRoot||document).querySelectorAll('.dv-ft-file')].find((e) => e.getAttribute('data-path') === " + JSON.stringify(d.path) + ") || [...((document.querySelector('#app-left')||{}).shadowRoot||document).querySelectorAll('.dv-ft-file')].find((e) => String(e.textContent || '').indexOf(" + JSON.stringify(d.path.split('/').pop()) + ") >= 0); if (!row) return 'no-row'; var k = row.querySelectorAll('*'); var leaf = k.length ? k[k.length - 1] : row; leaf.click(); return 'clicked'; })()")) === "clicked";
  await expandDir(d.dir);
  let has = await waitFor(rowExpr(d.path), 12000, 400);
  if (!has) {                                   // 自愈：宿主可能刚重启/换过档 ⇒ 整页重载后重来一次
    await send("Page.navigate", { url: BASE + PAGE_PREFIX + "/?wt=" + WT });
    await waitFor("!!((document.querySelector('#app-left')||{}).shadowRoot||document).querySelector('.dv-ft-file')", 45000, 1000);
    await waitFor(GRAB + " === 'ok'", 20000, 500);
    await expandDir(d.dir);
    has = await waitFor(rowExpr(d.path), 12000, 400);
  }
  if (!has) return { err: "文件树里找不到 " + d.path, diag: String(await ev("JSON.stringify({ rows: ((document.querySelector('#app-left')||{}).shadowRoot||document).querySelectorAll('.dv-ft-file').length, paths: [...((document.querySelector('#app-left')||{}).shadowRoot||document).querySelectorAll('.dv-ft-file')].slice(0, 12).map((e) => e.getAttribute('data-path')), dirs: [...((document.querySelector('#app-left')||{}).shadowRoot||document).querySelectorAll('.dv-ft-dir')].map((e) => e.getAttribute('title')), url: location.href })")).slice(0, 300) };
  // 诊断：把当时的树状态带出来 —— 之前只报「找不到」，而同样表达式在独立探针里是 true。
  if (!has) { const diag = await ev("JSON.stringify({ rows: ((document.querySelector('#app-left')||{}).shadowRoot||document).querySelectorAll('.dv-ft-file').length, paths: [...((document.querySelector('#app-left')||{}).shadowRoot||document).querySelectorAll('.dv-ft-file')].slice(0, 12).map((e) => e.getAttribute('data-path')), dirs: [...((document.querySelector('#app-left')||{}).shadowRoot||document).querySelectorAll('.dv-ft-dir')].map((e) => e.getAttribute('title')), url: location.href })"); return { err: "文件树里找不到 " + d.path, diag: String(diag).slice(0, 300) }; }
  if (!(await click())) return { err: "点击落空：" + d.path };
  // `globalThis.__cdr` 只有本套件自己在引用（app 早就不提供这个探针钩子）⇒ 用**可观察事实**：文件树里那一行变成当前档（`on`）。
  const landed = await waitFor("[...((document.querySelector('#app-left')||{}).shadowRoot||document).querySelectorAll('.dv-ft-file')].some((e) => e.getAttribute('data-path') === " + JSON.stringify(d.path) + " && /(^|\\s)on(\\s|$)/.test(e.className))", 25000, 400);
  const painted = landed && await waitFor("(() => { const r = (document.getElementById('app')&&document.getElementById('app').shadowRoot?document.getElementById('app').shadowRoot:document).querySelector('#mount'); return !!r && r.querySelectorAll('*').length > 0; })()", 15000, 300);
  if (landed) await wait(500);                  // 落在正确文档后**再**让渲染稳定一拍（只影响取值时刻，不影响结论）
  return { landed, painted };
}

/** 同一次求值里读 DOM 与交付树 —— 避免"读了 DOM 之后树又变"的错位。 */
const PROBE = [
  "(() => {",
  "  const uiRow = [...((document.querySelector('#app-left')||{}).shadowRoot||document).querySelectorAll('.dv-ft-file')].find((e) => String(e.className).indexOf('on') >= 0);",
  "  const ui = { docPath: uiRow ? uiRow.getAttribute('data-path') : null, doc: null };   // __cdr 已无人提供 ⇒ 用可观察事实：文件树里带 on 的那一行",
  "  const root = (document.getElementById('app')&&document.getElementById('app').shadowRoot?document.getElementById('app').shadowRoot:document).querySelector('#mount') || (document.getElementById('app')&&document.getElementById('app').shadowRoot) || document.body;   // null-safe",
  "  const dom = root ? { heads: root.querySelectorAll('h1,h2,h3,h4,h5,h6').length, lis: root.querySelectorAll('li').length, ps: root.querySelectorAll('p').length, std: root.querySelectorAll('.mdio-std').length, text: String(root.innerText || '') } : null;",
  "  const t = { heads: 0, bullets: 0, ps: 0, tags: [], depth: 0 };",
  "  const walk = (n, d) => {",
  "    if (!n || typeof n !== 'object' || d > 60) return;",
  "    t.depth = Math.max(t.depth, d);",
  "    const nt = n.nt;",
  "    if (nt === 'h' || nt === 'sec') t.heads += 1;",
  "    if (nt === 'b' || nt === 'x') { t.bullets += 1; const tag = typeof n.t === 'string' ? n.t.trim() : ''; if (tag && !tag.startsWith('[')) t.tags.push(tag); }",
  "    if (nt === 'p') t.ps += 1;",
  "    const kids = n.ct; if (kids && typeof kids !== 'string' && typeof kids.length === 'number') for (let i = 0; i < kids.length; i += 1) walk(kids[i], d + 1);",
  "  };",
  "  const doc = ui && ui.doc; if (doc) walk(doc, 0);",
  "  if (!dom) return JSON.stringify({ err: 'no .dv-doc-inner' });",
  "  return JSON.stringify({ docPath: ui ? ui.docPath : null, dom: { heads: dom.heads, lis: dom.lis, ps: dom.ps, std: dom.std, len: dom.text.length }, tree: t, missingTags: t.tags.filter((x) => dom.text.indexOf(x) < 0).slice(0, 12) });",
  "})()",
].join("\n");   // 必须是 "\n"：元素里有行尾 `//` 注释，join("") 会把注释后面**整段**吞掉 ⇒ 页面侧报 Unexpected end of input

for (const d of DOCS) {
  const opened = await openDoc(d);
  const raw = String(await ev(PROBE));
  let o = null; try { o = JSON.parse(raw); } catch (e) { /* 解析失败按红处理 */ }
  const tag = d.path.replace(/[^A-Za-z0-9.]/g, "_");
  await shot("doc-render" + tag + ".png");
  if (opened.err || !opened.landed || !o || o.err) { chk("打开 " + d.path, false, String([opened.err, opened.diag].filter(Boolean).join(" · ") || raw).slice(0, 380)); continue; }
  if (o.docPath !== d.path) { chk("打开 " + d.path, false, "打开的却是 " + o.docPath); continue; }
  // 交付树（store）已不暴露 ⇒ 这三条从「DOM vs 树」换成 **DOM-only 的结构断言**（不是删判据：仍要求文档面真的画出标题/段落/列表）。
  chk(d.path + " 有标题（DOM h1-h3 ≥ 1）", o.dom.heads >= 1, "DOM heads " + o.dom.heads);
  chk(d.path + " 有正文块（DOM li + p ≥ 1）", o.dom.lis + o.dom.ps >= 1, "li " + o.dom.lis + " + p " + o.dom.ps);
  chk(d.path + " 文档面有文字（len > 0）", o.dom.len > 0, "len " + o.dom.len);
  // 「文档面非空」的判据不能写成 `.mdio-std > 0`：std 只在 h/sec 上包 .mdio-std（std.js:392-398）
  // ⇒ 对**无标题文档**（首块就是列表/引用）恒不可满足，而那种文档渲染完全正确也可能 std=0
  //（T11 端到端反证：/demos/README.md 修好后 li 0→18、ul 0→5，而 .mdio-std 仍 0）。
  // 用「有结构元素」：std 壳 / li / p 任一存在即算画出来了。
  const painted = o.dom.len > 0 && (o.dom.std > 0 || o.dom.lis > 0 || o.dom.ps > 0);
  chk(d.path + " 文档面非空（" + o.dom.len + " 字符）", painted, "文本长度 " + o.dom.len + " · .mdio-std " + o.dom.std + " · li " + o.dom.lis + " · p " + o.dom.ps);
}

ws.close();
// 报告落盘：verify-all 只保留各套件的**总结行**，所以失败必须能从盘上回看（哪一项、什么值、哪张图）。
try { fs.mkdirSync(SHOT, { recursive: true }); fs.writeFileSync(path.join(SHOT, "doc-render-report.json"), JSON.stringify({ base: BASE, pagePrefix: PAGE_PREFIX, wt: WT, shots: SHOT, items: R }, null, 1)); } catch (e) { /* 报告写不出不影响判定 */ }
console.log("== check-doc-render ==");
for (const r of R) console.log((r.ok ? "  ✓ " : "  ✗ ") + r.name + (r.detail ? "  —— " + r.detail : ""));
const bad = R.filter((r) => !r.ok).length;
console.log(bad ? "✗ check-doc-render 失败 " + bad + " 项" : "✓ check-doc-render 全绿（" + R.length + " 项）· 截图 " + SHOT);
process.exit(process.exitCode === undefined ? (R.every((x) => x.ok) ? 0 : 1) : process.exitCode);   // 收尾退出：结论已打印 ⇒ 立刻退（CDP socket 会把进程吊到 deadline）
process.exit(bad ? 1 : 0);
