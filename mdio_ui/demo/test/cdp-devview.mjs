// cdp-devview —— devview 全功能清单的自动判据（CDP 真机：开页/点击/主题/console/写回/切档/开发者模式切换）。
// 用法：先起 gateway(8011) + esmx dev(5178) + headless Chrome(9222)，再 node test/cdp-devview.mjs
// 覆盖的三类「截图才能看见」的回归：
//   L1 文件树项被 flex 压成 0 高（DOM 有、看不见）→ 断言每项高度 > 4
//   L2 std 容器自带留白导致节节叠加大空白 → 断言 h1 底 → 下一节顶 < 150px
//   L3 产品单页残留开发状态行 rev= → 断言 appview 无 .dv-rev
import { writeFileSync } from "node:fs";
import { installGuards, makeEv, armWatchdog, withCallTimeout, guardTarget } from "./lib/watchdog.mjs";   // 超时 / 卡死检测 / 目标守卫
//  默认端口 9223：9222 是**业主的 Chrome**（连它必须显式 ALLOW_OWNER_CDP=1）。
const CDP = guardTarget(process.env.CDP_URL || "http://127.0.0.1:9223", "cdp-devview");
armWatchdog("cdp-devview", 65000)   /* = 65s */;   // 实测 max=32.3s（本机 2 跑）⇒ ×2=64.6s，取下限 10s ⇒ 65s
installGuards("cdp-devview");   // 未捕获异常 / 未处理拒绝 ⇒ 打印并 exit 4（不留「跑不完」）
const BASE = process.env.DEMO_URL || "http://127.0.0.1:5178";
const GW = process.env.GW_URL || "http://127.0.0.1:8011";
const R = [];
const chk = (id, name, ok, detail) => R.push({ id, name, ok: !!ok, detail: String(detail == null ? "" : detail).slice(0, 140) });
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
// close_old_tab 纪律（SOP）：遗留 tab 的 wasm worker 会争抢，先关掉除 about:blank 外的所有页
for (const t of await (await fetch(CDP + "/json")).json()) {
  if (t.type === "page" && (t.url || "").indexOf("about:blank") !== 0) { try { await fetch(CDP + "/json/close/" + t.id); } catch (e) { /* ignore */ } }
}
await wait(500);
const page = await (await fetch(CDP + "/json/new?" + encodeURIComponent(BASE + "/"), { method: "PUT" })).json();
const ws = new WebSocket(page.webSocketDebuggerUrl);
let id = 0; const pend = new Map();
const rawSend = (method, params) => new Promise((res, rej) => { const i = ++id; pend.set(i, { res, rej }); ws.send(JSON.stringify({ id: i, method, params })); });
const send = withCallTimeout("cdp-devview", rawSend);   // 单次 CDP 调用 20s 超时
ws.addEventListener("message", (ev) => { try { const m = JSON.parse(ev.data); if (m.id && pend.has(m.id)) { const p = pend.get(m.id); pend.delete(m.id); m.error ? p.rej(new Error(m.error.message)) : p.res(m.result); } } catch (e) { /* ignore */ } });
await new Promise((r) => ws.addEventListener("open", r));
await send("Runtime.enable"); await send("Page.enable");
const pageErrs = [];   // 页面异常账（Runtime.evaluate 的 exceptionDetails）
const ev = makeEv(send, "cdp-devview", pageErrs);
// 文本类探针：**文档文本 + 影子根文本**合并（B 路线下 app 内容在 `#app` 影子树里，`document.body.innerText` 看不到它）。
const body = () => ev("((document.body.innerText||'') + ' ' + (((document.getElementById('app')||{}).shadowRoot||{}).textContent||''))");
// 输入在 footer 的 pseudo console（旧壳 LogPanel:261-284），执行 = 回车（旧壳 onPressEnter）
const setVal = (v) => ev("(() => { const i = document.querySelector('.dv-footer .dv-exec'); const s = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set; s.call(i, " + JSON.stringify(v) + "); i.dispatchEvent(new Event('input', { bubbles: true })); return i.value; })()");
const pressEnter = () => ev("(() => { const i = document.querySelector('.dv-footer .dv-exec'); i.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })); return true; })()");
const execHttp = async (code) => { const r = await fetch(GW + "/exec", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ code, active_doc: "", wt_uid: "genesis" }) }); const j = await r.json(); return String(j.result == null ? "" : j.result); };
const until = async (fn, n = 30) => { for (let i = 0; i < n; i++) { await wait(1000); if (await fn()) return true; } return false; };

// 0 测试专用 scratch 文档：**所有写回/打字测试只在它上面做**（绝不碰 dashboard.md 等真实文档）
// 用已存在的测试文档当 scratch（`/>` 只能写已存在的文件；本网关无 __reset__ 钩子）
const SCRATCH = "/sop_probe.md";
const SCRATCH_DOC = "# SCRATCH\\n\\n## SEC\\n\\n- 说明: base\\n";
await execHttp(JSON.stringify(SCRATCH_DOC) + " | /> " + SCRATCH);

// A 壳与主题
await until(async () => (await ev("!!document.querySelector('.dv-wtbtn')")), 60);   // 左栏首行 = wt 切换按钮（旧「项目 / DEMOS」的位置）
// 页面默认是**产品面**（DevStore.devMode 默认 false，业主口径）；本套判据测开发者视图 ⇒ 显式切过去，不依赖默认值。
// 重载/换面后 devMode 会回默认，故各处需重新 ensure。
const ensureDev = async () => {
  if (await ev("!!(document.querySelector('.dv-root')||{classList:{contains:()=>false}}).classList.contains('is-product')")) {
    await ev("[...document.querySelectorAll('.dv-toggle')].find((b) => b.textContent.indexOf('开发者模式') >= 0)?.click()");
    await wait(900);
  }
};
await ensureDev();
// 旧壳 header 内容：文件名 + 全路径 + 面板开关 + 连接徽章 + 开发者模式/主题开关（无 brand、无 wt 徽章——wt 在左栏切换器里）
// 结构变更（业主要求）：三个开关并成一组 [Monaco][Log][开发者模式]；Monaco/Log 只在开发者模式打开时显现。
// ⇒ 计数从"2 个 .dv-toggle + 2 个 .dv-switch"变成"组内 3 个 .dv-toggle + 1 个 .dv-switch（主题）"。
chk("A1", "header（文件名/路径/一组两开关/连接/主题）", await ev("!!document.querySelector('.dv-filetag') && document.querySelectorAll('.dv-header .dv-seg .dv-toggle').length === 2 && !!document.querySelector('.dv-conn') && document.querySelectorAll('.dv-switch').length === 1"), await ev("(document.querySelector('.dv-header')?.innerText || '').replace(/\\n/g, ' | ').slice(0, 90)"));
const segLabels = await ev("JSON.stringify([...document.querySelectorAll('.dv-seg .dv-toggle')].map((b) => b.textContent.trim()))");
chk("A1b", "两开关一组且顺序 [Log][开发者模式]", segLabels === JSON.stringify(["Log", "开发者模式"]), segLabels);
await ev("[...document.querySelectorAll('.dv-toggle')].find((b) => b.textContent.indexOf('开发者模式') >= 0)?.click()"); await wait(450);
const offLabels = await ev("JSON.stringify([...document.querySelectorAll('.dv-seg .dv-toggle')].map((b) => b.textContent.trim()))");
const restStill = await ev("!!document.querySelector('.dv-seg .dv-toggle') && !!document.querySelector('.dv-conn') && document.querySelectorAll('.dv-switch').length === 1");
chk("A1c", "关开发者模式 ⇒ Monaco/Log 消失（开发者模式钮与连接徽章仍在）", offLabels === JSON.stringify(["开发者模式"]) && restStill, offLabels);
await ev("[...document.querySelectorAll('.dv-toggle')].find((b) => b.textContent.indexOf('开发者模式') >= 0)?.click()"); await wait(450);
const bgDark = await ev("getComputedStyle(document.body).backgroundColor");
chk("A2", "默认暗色", bgDark === "rgb(15, 15, 18)", bgDark);
await ev("[...document.querySelectorAll('.dv-switch')].at(-1)?.click()"); await wait(400);
// A3/A3b：真测切换三段式：
//   旧写法在第 56 行断言"亮色"、第 57 行才点击 ⇒ 它其实在断言**初始主题**，而本应用**故意默认暗色**
//   （见 `entry.client.ts:12`：默认暗色，旧壳同款 `.dark` 机制）⇒ 旧期望与产品现状不符（与 D2 同类的过期期望）。
//  **不能依赖进入时的主题**（踩到）：CDP 复用标签页 ⇒ 上一轮把主题留在亮色，本轮"初始=暗色"必然失败。
// 先**幂等归零**到暗色，再测切换能力（这才是本项的意图）。
for (let i = 0; i < 3 && !(await ev("document.body.classList.contains('dark')")); i++) { await ev("[...document.querySelectorAll('.dv-switch')].at(-1)?.click()"); await wait(300); }
chk("A3", "归零后主题 = 暗色", (await ev("getComputedStyle(document.body).backgroundColor")) === "rgb(15, 15, 18)", await ev("document.body.className"));
await ev("[...document.querySelectorAll('.dv-switch')].at(-1)?.click()"); await wait(400);
chk("A3b", "主题切换 → 亮色", (await ev("getComputedStyle(document.body).backgroundColor")) === "rgb(255, 255, 255)", await ev("document.body.className"));
await ev("[...document.querySelectorAll('.dv-switch')].at(-1)?.click()"); await wait(400);
chk("A3c", "再切换 → 回暗色", (await ev("getComputedStyle(document.body).backgroundColor")) === "rgb(15, 15, 18)", await ev("document.body.className"));
const sizes = JSON.parse(await ev("JSON.stringify({ t: Math.round(document.querySelector('.dv-left').getBoundingClientRect().width), r: Math.round((document.querySelector('.dv-log-col') || { getBoundingClientRect: () => ({ width: 0 }) }).getBoundingClientRect().width), h: Math.round(document.querySelector('.dv-header').getBoundingClientRect().height) })"));
chk("A4", "布局尺寸 640/340/36（±3px 边框；左格 = 合并 app 整格宽）", Math.abs(sizes.t - 640) <= 3 && Math.abs(sizes.r - 340) <= 3 && Math.abs(sizes.h - 36) <= 3, JSON.stringify(sizes));
// A5 左上角 wt 切换（旧 devview 的「项目 / DEMOS」等价物）：按钮显示当前 wt，点开是清单；fileview 只列当前 wt
await ev("document.querySelector('.dv-wtbtn')?.click()");
await wait(300);
const wtRows = JSON.parse(await ev("JSON.stringify({ btn: document.querySelector('.dv-wtbtn .nm')?.textContent, inHeader: !!document.querySelector('.dv-header .dv-wtbtn'), rows: [...document.querySelectorAll('.dv-wtitem .nm')].map((d) => d.textContent), active: document.querySelector('.dv-wtitem.on .nm')?.textContent, menuN: document.querySelectorAll('.dv-wtitem').length, filesInSide: ((document.querySelector('#app-left')||{}).shadowRoot||{querySelectorAll:()=>[]}).querySelectorAll('.dv-ft-file').length, projRows: document.querySelectorAll('.dv-proj').length, foot: !!document.querySelector('.dv-side-foot') })") || "{}");
await ev("document.querySelector('.dv-wtbtn')?.click()");   // 收起菜单
chk("A5", "wt 切换在 header（当前 wt + 清单）+ fileview 无自带头脚", wtRows.btn === "genesis" && wtRows.menuN >= 2 && wtRows.active === "genesis" && wtRows.projRows === 0 && wtRows.filesInSide > 0 && wtRows.inHeader && !wtRows.foot, JSON.stringify(wtRows));

// B 文件树（L1 回归）
const vis = JSON.parse(await ev("JSON.stringify((() => { const sr = (document.querySelector('#app-left')||{}).shadowRoot; const rows = sr ? [...sr.querySelectorAll('.dv-ft-row.dv-ft-file')] : []; return { n: rows.length, visible: rows.filter((d) => d.getBoundingClientRect().height > 4).length }; })())"));
chk("B1", "[L1] 文件树项真可见（高度>4）", vis.n > 0 && vis.visible === vis.n, JSON.stringify(vis));
// 左栏 footer = 旧壳提示语（不带计数）；条目数在 app footer「文件 N」里核对
// fileview 不再自带 footer（用户口径：三行都横贯整页宽，fileview 只是 workspace 里的一列）
const footHint = await ev("document.querySelector('.dv-side-foot')?.textContent");   // 期望为 null（不该有）
const appFoot = await ev("document.querySelector('.dv-footer')?.innerText");
// 口径说明：侧栏只列 .md 且排 /.mds/（旧壳语义），app footer 的「文件 N」= 该 wt 作用域树条目数（日志 tree: entries N 为权威值）
// 注意：日志里有两处 tree 计数——WS 树（引擎推）与作用域树（fullTree）；这里取**带作用域注解**的那条
const treeN = Number(((await body()).match(/tree: entries (\d+)（wt=/) || [])[1] || "-1");
chk("B2", "fileview 无自带 footer · page footer 计数 = 作用域树条目数", !footHint && !!appFoot && treeN > 0 && appFoot.indexOf("文件 " + treeN) >= 0, "tree=" + treeN + " md=" + vis.n + " | " + String(appFoot).replace(/\n/g, " "));
await ev("((document.querySelector('#app-left')||{}).shadowRoot||{querySelector:()=>null}).querySelector('.dv-ft-row.dv-ft-file[data-path=\"/dashboard.md\"]')?.click()");
chk("B3", "点 dashboard.md → 打开 + active", await until(async () => (await ev("(((document.querySelector('#app-left')||{}).shadowRoot||{querySelector:()=>null}).querySelector('.dv-ft-row.on .nm')||{}).textContent")) === "dashboard.md" && (await body()).indexOf("rev=") >= 0));

// C 文档面（L2 回归）
const gap = await ev("(() => { const R = ((document.getElementById('app')||{}).shadowRoot||document); const h1 = R.querySelector('h1'); const hs = [...R.querySelectorAll('.mdio-std h2')]; if (!h1 || !hs.length) return -1; return Math.round(hs[0].getBoundingClientRect().top - h1.getBoundingClientRect().bottom); })()");
chk("C0", "[L2] 节间空白 < 150px", typeof gap === "number" && gap >= 0 && gap < 150, "gap=" + gap);
const ds = JSON.parse(await ev("JSON.stringify({ std: ((document.getElementById('app')||{}).shadowRoot||document).querySelectorAll('.mdio-std').length, h1: !!((document.getElementById('app')||{}).shadowRoot||document).querySelector('h1'), rev: (document.body.innerText.match(/rev=(\\d+)/) || [])[1], region: (((document.getElementById('app')||{}).shadowRoot ? document.getElementById('app').shadowRoot.textContent : document.body.innerText)||'').indexOf('◆ 引用') >= 0, builtin: document.body.innerText.indexOf('（bl）') >= 0 })"));
// C1（改）：那行 rev/认领表/mods 是自造物——现在断言两侧都**不得**出现自造状态行
const appRevLine = await ev("!!document.querySelector('.dv-app .dv-rev')");
chk("C1", "无自造状态行（.dv-rev 已删）", !appRevLine, JSON.stringify(ds));
chk("C2", "std 渲染（多节实例 + h1）", ds.std > 1 && ds.h1, JSON.stringify(ds));
chk("C3", "REGION 认领命中", ds.region);
chk("C4", "嵌套节也走 std（无内置直出标记）", !ds.builtin);

// D console
chk("D1", "footer console 空值 + 旧 placeholder", (await ev("document.querySelector('.dv-footer .dv-exec')?.value")) === "" && String(await ev("document.querySelector('.dv-footer .dv-exec')?.getAttribute('placeholder')")).indexOf("mdio exec，如:") >= 0);
await setVal("$ /.mds/ui.md::mods");
await pressEnter();
await wait(2500);
// 结果落在 footer 的 pseudo console（.dv-lastout），app 栏不再有输出块；日志同一份走 CONSOLE<:
const out = await ev("document.querySelector('.dv-footer-result')?.textContent");
const appHasOut = await ev("!!document.querySelector('.dv-app .dv-out')");
chk("D2", "执行读指令 → 结果在 footer（app 栏无输出块）+ log 行", out && (out.indexOf("std: ui_std/std") >= 0 || out.indexOf("std: .ui::std") >= 0 || out.indexOf("std: ui:std") >= 0) && !appHasOut && ((await body()).indexOf("CLI→ $ /.mds/ui.md::mods") >= 0), String(out).slice(0, 60));
// 写回自动更新：**只在测试自建的 scratch 文档上做**（绝不写真实文档——G2/D3 曾污染 dashboard.md）
// 先把 scratch 文档开在 devview 里（P 帧才会落到当前打开的文档上）
await ev("((document.querySelector('#app-left')||{}).shadowRoot||{querySelector:()=>null}).querySelector('.dv-ft-file[data-path=\"/sop_probe.md\"]')?.click()");
await until(async () => (await body()).indexOf("SCRATCH") >= 0, 20);
const SELF = SCRATCH + "::SEC._b";
const orig = String(await execHttp("$ " + SELF + " -pj ct")).trim();
const needle = "DVPROBE" + Date.now().toString().slice(-4);
await setVal(JSON.stringify(needle) + " | /> " + SELF);
await pressEnter();
const updated = await until(async () => (await body()).indexOf(needle) >= 0, 12);
const revAfter = (await ev("document.querySelector('.dv-footer-bar')?.innerText || ''")).match(/rev (\d+)/);
await execHttp(JSON.stringify(orig) + " | /> " + SELF);
chk("D3", "console 写 → P 帧自动更新（页面出现新值）", updated, "rev=" + (revAfter && revAfter[1]));

// E log
const logHead = await ev("document.querySelector('.dv-log .dv-pane-head')?.textContent");
// 面板头照旧壳：标题「MDIO 管线日志」+ 条数徽章 + fps 徽章（计数徽章 = DOM 行数）
// 同一帧内取两个数，避免日志继续增长造成的竞态
const logStat = JSON.parse(await ev("JSON.stringify({ dom: document.querySelectorAll('.dv-log-line').length, badge: document.querySelector('.dv-log .dv-count')?.textContent, fps: document.querySelector('.dv-log .dv-fps')?.textContent })") || "{}");
const logLines = logStat.dom;
const countBadge = logStat.badge;
const fpsBadge = logStat.fps;
chk("E1", "log 头：标题 + 计数徽章=行数 + fps 徽章", !!logHead && logHead.indexOf("MDIO 管线日志") >= 0 && String(countBadge) === String(logLines) && /fps$/.test(String(fpsBadge)), "count=" + countBadge + " dom=" + logLines + " fps=" + fpsBadge);
chk("E2", "boot 日志齐全", await ev("['tree: entries','boot: decl','boot: claims','AST F-snapshot'].every((k) => document.body.innerText.indexOf(k) >= 0)"), "lines=" + logLines);

// K Perf 卡片（右列顶部：[Perf] 在上、[log] 在下；数据与日志同源）
const perfInfo = JSON.parse(String(await ev("JSON.stringify((() => { const p = document.querySelector('.dv-perf'); const l = document.querySelector('.dv-log-col .dv-log'); const badge = document.querySelector('.dv-log .dv-fps'); if (!p) return null; const pr = p.getBoundingClientRect(); const lr = l ? l.getBoundingClientRect() : null; return { text: (p.innerText || '').replace(/\\n/g, ' | '), top: Math.round(pr.top), logTop: lr ? Math.round(lr.top) : null, width: Math.round(pr.width), badge: badge ? badge.innerText : null }; })())")) || "null");
chk("K1", "Perf 卡片在 log 列顶部（[Perf] 上 / [log] 下）", !!perfInfo && perfInfo.logTop != null && perfInfo.top < perfInfo.logTop && /\d+ fps/.test(perfInfo.text || ""), JSON.stringify(perfInfo));
chk("K2", "Perf 显示 fps + 帧计数（D 段写入后 P ≥ 1）；log 徽章读同一份采样", !!perfInfo && /P [1-9]/.test(perfInfo.text || "") && !!perfInfo.badge && /fps/.test(perfInfo.badge), JSON.stringify({ text: perfInfo && perfInfo.text, badge: perfInfo && perfInfo.badge }));
await ev("document.querySelector('.dv-perf')?.click()");
await wait(250);
const collapsed = await ev("!!document.querySelector('.dv-perf.is-collapsed')");
const subHidden = await ev("(() => { const el = document.querySelector('.dv-perf .dv-perf-row.sub'); return el ? getComputedStyle(el).display === 'none' : true; })()");
await ev("document.querySelector('.dv-perf')?.click()");
await wait(200);
const expanded = await ev("!!document.querySelector('.dv-perf') && !document.querySelector('.dv-perf.is-collapsed')");
chk("K3", "点击折叠 / 再点展开", collapsed === true && subHidden === true && expanded === true, JSON.stringify({ collapsed, subHidden, expanded }));

// K4（回归守卫）：写入后 **incr 与 render 都要有数值** —— 打点写在 await 之后，
// 补丁先到时标记还没立 ⇒ Perf 只剩 render、日志从不出现 patch_latency。
const segs = JSON.parse(String(await ev("JSON.stringify((() => { const t = (document.querySelector('.dv-perf') || {}).innerText || ''; const log = (document.querySelector('.dv-log-col .dv-log') || {}).innerText || ''; return { perf: t.replace(/\\n/g, ' | '), hasIncr: /incr \\d/.test(t), hasRender: /render \\d/.test(t), hasPatchLatency: /patch_latency\\s+\\d/.test(log) }; })())")) || "{}");
chk("K4", "写入后 Perf 的 incr 与 render 都有值；日志出现 patch_latency（打点时机回归）", segs.hasIncr === true && segs.hasRender === true && segs.hasPatchLatency === true, JSON.stringify(segs));

// H6/H7 命令历史（「console 回车/历史」）：↑ 往回翻、↓ 往新翻、越界回草稿、落 sessionStorage
const pressKey = async (key) => {
  await ev("(() => { const i = document.querySelector('.dv-footer .dv-exec'); if (!i) return null; i.focus(); i.dispatchEvent(new KeyboardEvent('keydown', { key: '" + key + "', code: '" + key + "', keyCode: " + (key === "ArrowUp" ? 38 : 40) + ", bubbles: true })); })()");
  await wait(250);   // React 提交后再读值（dispatch 同拍读到的是提交前的值）
  return String(await ev("(document.querySelector('.dv-footer .dv-exec') || {}).value"));
};
const CMD_A = "$ /.mds/ui.md::mods";
const CMD_B = "$ /.mds/ui.md";
await setVal(CMD_A); await pressEnter(); await wait(1500);
await setVal(CMD_B); await pressEnter(); await wait(1500);
const afterRun = String(await ev("(document.querySelector('.dv-footer .dv-exec') || {}).value"));
const up1 = String(await pressKey("ArrowUp"));
const up2 = String(await pressKey("ArrowUp"));
const down1 = String(await pressKey("ArrowDown"));
const down2 = String(await pressKey("ArrowDown"));
chk("H6", "命令历史：回车清空 + ↑ 翻上一条/A ↑↑ / ↓ 回退 / ↓↓ 回草稿", afterRun === "" && up1 === CMD_B && up2 === CMD_A && down1 === CMD_B && down2 === "", JSON.stringify({ afterRun, up1, up2, down1, down2 }));
await send("Page.reload", { ignoreCache: false });
let reborn = false;
for (let k = 0; k < 40; k += 1) { await wait(900); if (await ev("!!document.querySelector('.dv-footer .dv-exec')")) { reborn = true; break; } }
await wait(2500);
const upAfterReload = String(await pressKey("ArrowUp"));
chk("H7", "历史落 sessionStorage：整页重载后 ↑ 仍能翻出上一条", reborn === true && upAfterReload === CMD_B, JSON.stringify({ reborn, upAfterReload }));

// F 开发者模式切换（L3 回归）——先切回真实文档 dashboard.md（产品页渲染当前打开的文档）
await ev("((document.querySelector('#app-left')||{}).shadowRoot||{querySelector:()=>null}).querySelector('.dv-ft-file[data-path=\"/dashboard.md\"]')?.click()");
await until(async () => (await body()).indexOf("DASHBOARD") >= 0, 20);
await ensureDev();   // H7 整页重载后 devMode 回默认（产品面），先确保在开发面；下面这一下点击 = 切到产品面
await ev("[...document.querySelectorAll('.dv-toggle')].find((b) => b.textContent.indexOf('开发者模式') >= 0)?.click()");
const appOk = await until(async () => (await ev("location.href")).indexOf("app=") >= 0 && (await body()).indexOf("DASHBOARD") >= 0, 30);
const appD = JSON.parse(await ev("JSON.stringify({ rev: !!document.querySelector('.dv-rev'), std: !!((document.getElementById('app')||{}).shadowRoot||document).querySelector('.mdio-std'), h1: !!((document.getElementById('app')||{}).shadowRoot||document).querySelector('h1'), bg: getComputedStyle(document.body).backgroundColor })"));
chk("F1", "开发者模式切换 → ?app= → Appview 渲染", appOk, await ev("location.href"));
chk("F2", "[L3] 产品单页无开发状态行", !appD.rev, JSON.stringify(appD));
chk("F3", "产品单页暗色 + std + h1", appD.std && appD.h1 && appD.bg === "rgb(15, 15, 18)", JSON.stringify(appD));

// G mona：Monaco edit → CRDT → md 渲染更新（最低闭环；旧 devview 的 monaco 块等价物）
await ev("location.href = " + JSON.stringify(BASE + "/?wt=genesis"));
await until(async () => (await ev("!!document.querySelector('.dv-wtbtn')")), 60);   // 左栏首行 = wt 切换按钮（旧「项目 / DEMOS」的位置）
// 打开 scratch 文档（不是真实文档——打字会写进文档本体；它已在第 0 步建好，无需 reload）
await until(async () => (await ev("!!document.querySelector('.dv-wtbtn')")), 30);
await ev("((document.querySelector('#app-left')||{}).shadowRoot||{querySelector:()=>null}).querySelector('.dv-ft-file[data-path=\"/sop_probe.md\"]')?.click()");
await until(async () => (await ev("!!((document.getElementById('app')||{}).shadowRoot||document).querySelector('.mdio-std')")), 30);
// 确保 Monaco 面板是开的（缺省已开；只在关着时点一下）
await ev("(() => { const b = [...document.querySelectorAll('.dv-toggle')].find((x) => x.textContent === 'Monaco'); if (b && b.className.indexOf('on') < 0) b.click(); return b ? b.className : null; })()");
let monaOk = false;
for (let i = 0; i < 40 && !monaOk; i++) { await wait(1000); monaOk = !!(await ev("!!((document.querySelector('#app-left')||{}).shadowRoot||{querySelector:()=>null}).querySelector('.monaco-editor')")); }
chk("G1", "mona 面板挂载 Monaco 编辑器", monaOk);
if (monaOk) {
  const revBefore = Number((await ev("document.querySelector('.dv-footer-bar')?.innerText || ''")).match(/rev (\d+)/)?.[1] || "0");
  // 打字走 mdio-sop-edit 驱动口：Monaco 0.57 在 Chromium 走 EditContext，CDP 的 Input.insertText 不进模型；
  // 该事件由 LeftApp 的 MonacoHost 监听 → model.pushEditOperations → 同一条 onDidChangeContent → CRDT 发送链。
  const driveEdit = async (line, from, to, text) => ev("(() => { const h = ((document.querySelector('#app-left')||{}).shadowRoot||{querySelector:()=>null}).querySelector('[data-mdio-mona]'); if (!h) return 'no-host'; h.dispatchEvent(new CustomEvent('mdio-sop-edit', { detail: " + JSON.stringify({ line, from, to, text }) + " })); return 'ok'; })()");
  const drive1 = await driveEdit(1, 10, 10, "\n\n- MONACDP1: 打字");
  let typed = false;
  for (let i = 0; i < 20 && !typed; i++) { await wait(1000); typed = (await body()).indexOf("MONACDP1") >= 0; }
  const revAfter = Number((await ev("document.querySelector('.dv-footer-bar')?.innerText || ''")).match(/rev (\d+)/)?.[1] || "0");
  chk("G2", "mona 打字 → CRDT 写入（rev 递增）", typed && revAfter > revBefore, "rev " + revBefore + "→" + revAfter + " drive=" + drive1);
  chk("G3", "mona 打字 → md 渲染面板同步出现新文本", await ev("(((document.getElementById('app')||{}).shadowRoot ? document.getElementById('app').shadowRoot.textContent : document.body.innerText)||'').indexOf('MONACDP1') >= 0"));
  // G3b 深层节点（二级节内的列表项）打字：走「子节点自身订阅」路径 ⇒ 渲染面板同样更新。
  // 与 G3（根层）配对，防住「快照版本 + memo 屏障」两条机制中的任一条被破坏。
  // 行号取引擎真值（不靠屏幕坐标）：找到「说明」行，在行尾追加 ZZ1。
  const curDoc = String(await execHttp("$ /sop_probe.md"));
  const docLines = curDoc.split(String.fromCharCode(10));
  const lineNo = docLines.findIndex((l) => l.indexOf("说明") >= 0) + 1;
  const fromCol = lineNo > 0 ? String(docLines[lineNo - 1] || "").length + 1 : 1;
  const drive2 = lineNo > 0 ? await driveEdit(lineNo, fromCol, fromCol, "ZZ1") : "no-line";
  let deepTyped = false;
  for (let i = 0; i < 20 && !deepTyped; i++) { await wait(1000); deepTyped = (await body()).indexOf("ZZ1") >= 0; }
  chk("G3b", "深层节点（二级节内）打字 → 渲染面板同步", deepTyped, "line=" + lineNo + " drive=" + drive2);
  // G6（回声增长回归）：Monaco 的改动只能**单向**进引擎。若"远端应用"没置 isProcessingRemote，
  // onDidChangeContent 会把同一次修改再发回去 ⇒ 文档回声膨胀（实测曾把 stats.md 撑到 512KB、几百份副本，
  // 渲染只认其中一份 ⇒ 表现为"改了 Monaco，render 不更新"）。这里钉住"打字后文档大小仍在正常量级"。
  const docLen = String(await execHttp("$ /sop_probe.md")).length;
  chk("G6", "打字后文档不膨胀（回声增长回归）", docLen > 0 && docLen < 20000, "文档 " + docLen + " 字节");
  // G5（正向 + 外部写入桥回归）：**exec 写**只产生 AST 帧，而 Monaco 会话只吃文本 op ⇒ 编辑器原本不动。
  // MonaPanel 的桥（帧到 ⇒ 取回全文 ⇒ 最小差异替换）负责补上 ⇒ 这里断言 Monaco 的 DOM 文本确实跟上了。
  // 关键点：写要来自**别的 peer**（footer CLI 的 exec），POST 的是整篇新文档 ⇒ 只会产生 AST 帧；
  // 若 Monaco 的 DOM 出现了新文本，那只能是桥（帧到 ⇒ 取回全文 ⇒ 最小差异替换）干的。
  const monaNow = async () => String(await ev("String([...(((document.querySelector('#app-left')||{}).shadowRoot||{querySelectorAll:()=>[]}).querySelectorAll('.view-lines'))].map((e)=>e.innerText).join(String.fromCharCode(10)))"));
  const mark = "FWDPROBE" + Date.now().toString().slice(-4);
  await setVal(JSON.stringify("# 外部写入\n\n## 面板\n\n- 标记: " + mark + "\n\n正文" + mark + "\n") + " | /> /sop_probe.md");
  await pressEnter();
  const fwdOk = await until(async () => (await monaNow()).indexOf(mark) >= 0, 25);
  chk("G5", "**exec 写（别的 peer）⇒ Monaco 跟随**（外部写入桥回归）", fwdOk, mark + " ｜ Monaco: " + (await monaNow()).replace(/\n+/g, " | ").slice(0, 60));
  // 日志面板是虚拟滚动（react-virtuoso），DOM 里只有可见行 ⇒ 读 logBus 环形缓冲（逻辑名可被页面 import map 解析）
  const logHas = await ev("(async () => { try { const m = await import('ui_core/logBus'); const t = m.logSnapshot().entries.map((e) => e.text).join(String.fromCharCode(10)); return t.indexOf('open doc=') >= 0 && t.indexOf('MC_UP') >= 0; } catch (e) { return false; } })()");
  chk("G4", "mona 装载/写入有日志", logHas);
}


// ═════ H 末段：footer 拖拽 / 开发者模式开关（放最后，避免影响前面各段的页面状态） ═════
await ensureDev();   // G 段 location 换过页 ⇒ devMode 回默认（产品面）；H 段测的是开发面
// H1 footer 顶边可拖 + 上限 20% 窗口高（用户口径：只有顶边那条线可拖）
const gripOk = await ev("!!document.querySelector('.dv-footer .dv-grip')");
const boxOf = async () => JSON.parse(String(await ev("(() => { const f = document.querySelector('.dv-footer'); if (!f) return 'null'; const r = f.getBoundingClientRect(); return JSON.stringify({ h: Math.round(r.height) }); })()")) || "null");
const gripPos = async () => JSON.parse(String(await ev("(() => { const g = document.querySelector('.dv-footer .dv-grip'); if (!g) return 'null'; const r = g.getBoundingClientRect(); return JSON.stringify({ x: Math.round(r.x + r.width / 2), y: Math.round(r.y + r.height / 2) }); })()")) || "null");
const before4 = await boxOf();
let g1 = await gripPos();
if (g1) {
  await send("Input.dispatchMouseEvent", { type: "mousePressed", x: g1.x, y: g1.y, button: "left", clickCount: 1 });
  await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: g1.x, y: 8, button: "left", buttons: 1 });
  await send("Input.dispatchMouseEvent", { type: "mouseReleased", x: g1.x, y: 8, button: "left", clickCount: 1 });
}
await wait(500);
const after4 = await boxOf();
const cap4 = Number(await ev("Math.round(innerHeight * 0.2)")) || 0;
chk("H1", "footer 顶边可拖 + 不超过 20% 窗口高", !!gripOk && !!before4 && !!after4 && after4.h > before4.h && after4.h <= cap4 + 2, JSON.stringify({ grip: !!gripOk, before: before4, after: after4, cap: cap4 }));
// 还原：拖回输入条高度
const g2 = await gripPos();
if (g2) {
  await send("Input.dispatchMouseEvent", { type: "mousePressed", x: g2.x, y: g2.y, button: "left", clickCount: 1 });
  await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: g2.x, y: 5000, button: "left", buttons: 1 });
  await send("Input.dispatchMouseEvent", { type: "mouseReleased", x: g2.x, y: 5000, button: "left", clickCount: 1 });
  await wait(400);
}
const back4 = await boxOf();
chk("H2", "拖回后高度回落（不粘住）", !!back4 && back4.h <= Math.max(48, (before4 ? before4.h : 48)) + 2, JSON.stringify({ before: before4, back: back4 }));
// H3 开发者模式：关掉 ⇒ 只留 header（左栏/中栏/右栏/footer 全消失）；再开 ⇒ 回来
const stage = async () => JSON.parse(String(await ev("JSON.stringify((() => { const vis = (s) => { const el = document.querySelector(s); if (!el) return null; const cs = getComputedStyle(el); if (cs.display === 'none' || cs.visibility === 'hidden') return false; const r2 = el.getBoundingClientRect(); return r2.width > 0 && r2.height > 0; }; return { side: vis('.dv-left'), mona: vis('.dv-left'), log: vis('.dv-log-col'), foot: vis('.dv-footer'), head: vis('.dv-filetag'), sideMounted: !!document.querySelector('.dv-left'), monaEditor: !!((document.querySelector('#app-left')||{}).shadowRoot||{querySelector:()=>null}).querySelector('.monaco-editor'), href: location.href.indexOf('app=') >= 0, face: (document.querySelector('.mdio-root')||{getAttribute:()=>''}).getAttribute('data-face'), origin: performance.timeOrigin }; })())")) || "{}");
const stOn = await stage();
await ev("[...document.querySelectorAll('.dv-toggle')].find((b) => b.textContent.indexOf('开发者模式') >= 0)?.click()");
await wait(1600);
const stOff = await stage();
// 面模型（业主再更正）：**devview 包裹 appview** —— devview = header + workspace[fileview mona log] + footer，`#app` 在 workspace 里被包围但**不属于 devview**；
// 开发者模式只控制 **workspace + footer 的可见性**，**header 永远显示**（所以关掉后 `head` 仍为 true）。
chk("H3", "关开发者模式 ⇒ 三栏与 footer 隐藏、**header 仍在**（非卸载）", stOn.side === true && stOn.foot === true && stOff.head === true && stOff.side === false && stOff.mona === false && stOff.log === false && stOff.foot === false && stOff.sideMounted === true, JSON.stringify({ on: stOn, off: stOff }));
await ev("[...document.querySelectorAll('.dv-toggle')].find((b) => b.textContent.indexOf('开发者模式') >= 0)?.click()");
await wait(1600);
const stBack = await stage();
chk("H4", "再开 ⇒ 三栏与 footer 回来（显隐就地）", stBack.side === true && stBack.mona === true && stBack.log === true && stBack.foot === true && stBack.head === true, JSON.stringify(stBack));
// H5 就地开关（关键回归）：**页面不重载** —— timeOrigin 不变 + Monaco 会话仍在 + URL 用 replaceState 表示
chk("H5", "开发者模式切换**不重载**（timeOrigin 不变 + Monaco 会话在 + URL 同步）", stOn.origin === stOff.origin && stOff.origin === stBack.origin && stOff.sideMounted === true && stOff.monaEditor === true && stBack.monaEditor === true, JSON.stringify({ origin: [stOn.origin, stOff.origin, stBack.origin], monaEditor: [stOn.monaEditor, stOff.monaEditor, stBack.monaEditor] }));

// 页面异常也进判据账（不只是一行日志）
R.push({ id: "PX", name: "无页面异常", ok: pageErrs.length === 0, detail: pageErrs.slice(0, 2).join(" | ").slice(0, 140) });
const failed = R.filter((r) => !r.ok);
for (const r of R) console.log((r.ok ? " ✓ " : " ✗ ") + r.id + " " + r.name + (r.ok ? "" : "  → " + r.detail));
console.log(failed.length ? "✗ cdp-devview 失败 " + failed.length + " 项" : "✓ cdp-devview 全绿（" + R.length + " 项）");
if (process.env.SHOT_DIR) {
  const s = await send("Page.captureScreenshot", { format: "png" });
  writeFileSync(process.env.SHOT_DIR + "/cdp-devview-last.png", Buffer.from(s.data, "base64"));
}
process.exit(failed.length ? 1 : 0);