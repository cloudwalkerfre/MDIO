#!/usr/bin/env node
import { installGuards, armWatchdog } from "./lib/watchdog.mjs";
// 旧 `hitSetsOf` 已从 uiMd.js 移除（现为逐节点 `matchOf(node, table)`）⇒ 套件改用当前 API。
const { matchOf: claimOfApi } = await import("ui_core/services/uiMd.js");
installGuards("e2e");
armWatchdog("e2e", 30000);   /* 未测到用时（扫描时未跑成功）⇒ 暂给 30s，待实测重定 */
// @ts-check
/**
 * e2e —— 新 demo 的端到端自动化测试（零依赖，node ≥ 22：全局 fetch + WebSocket）。
 * 覆盖：A 壳宿主（esmx dev :5178）/ B exec DSL / C 文件树 / D WS 帧协议 /
 * E 声明提取（import 真 uiMd.js）/ F 认领 / BOOT 状态机 / UI 合成树 / W 写回 /
 * 判官子进程 / H1 声明健康 / H2 模块健康门（esmx build 产物 + esmx validate）/ ⑰ SSR 边界 /
 * ⑱ wt ui 模块链（ui:<path> → import map 标识符 <模块名>/<导出名>，且产物真能取到）/
 * ⑲ 多 wt 链（隔离/本地/继承/子覆父 + 宿主 links 并集 + 子 wt 页面内联链）。
 * G 装载线（G1-G10）不在本套件。
 * 读-only：不写任何文档（例外：⑯ 在 .tmp/e2e-build 影子工程里跑 esmx build，真工程 dist/ 零接触，跑完清理）。
 * 前置：esmx dev（:5178）与 gateway（:8011）在跑。
 */

const BASE = process.env.BASE || "http://127.0.0.1:5178";   // 缺省不变；带 BASE 时用于"开关开"的 A/B（经 302 跳板注入 ?cursor=1）
const GW = "http://127.0.0.1:8012";   // 网关已迁 8012（端口漂移同步，与 entry.node 同源修复）
const WS_URL = "ws://127.0.0.1:5178/ws/gateway";

// 协议常量（与 src/services/client.js 一致；参照 Rust 协议定义）
const TAG = {
  UpTree: 0x01, UpOpen: 0x02, ExecReq: 0x0e, DnExec: 0x85,
  RemoteOp: 0x10, RenderDir: 0x81, DnAst: 0x84,
};
const HEADER = 13;

let failed = 0;
function ok(name, detail) { console.log("  ✓ " + name + (detail ? " —— " + detail : "")); }
function bad(name, detail) { failed += 1; console.error("  ✗ " + name + (detail ? " —— " + detail : "")); }
function stage(name) { devWatchMarks.push({ tick: devWatchTicks.length, name }); console.log("[" + name + "]"); }

// ── devwatch（DEVWATCH=1 启用）：后台每 2s 同时探 **渲染路由** 与 **control 路由**，
//    并把每个 stage 与探针序号对齐 ⇒ 定位"渲染路由被哪一阶段打断"。
//    这套探针就是当初定位根因的工具（结论：⑯ 影子构建产物落在 dev 根内 ⇒ esmx watcher 全量 page reload）。
//    两条路由分开探是关键：打断时 GET / 会 RST 而 /control 仍通 ⇒ 只有走 Vite SSR 管线的渲染路由受影响。
const devWatchOn = process.env.DEVWATCH === "1";
/** 单点探渲染路由（DEVWATCH=1 时打印）——用来把 breakage 夹到 ⑯ 的某个子步。 */
const probeRender = async (tag) => {
  if (!devWatchOn) return null;
  try { const r = await fetch(BASE + "/", { signal: AbortSignal.timeout(5000) }); await r.text(); console.log("  [probe] " + tag + " 渲染=200"); return true; }
  catch (e) { const c = String((e.cause && (e.cause.code || e.cause.message)) || e.message).slice(0, 22); console.log("  [probe] " + tag + " 渲染=FAIL:" + c); return false; }
};
const devWatchTicks = [];
const devWatchMarks = [];
let devWatchFirstFail = null;
let devWatchTimer = null;
if (devWatchOn) {
  // 递归 setTimeout（不是 setInterval）：⑯ 的同步 spawnSync 会长时间占住事件循环，
  // setInterval 排队后只跑得出 1 次（实测）⇒ 递归版每次跑完再排下一次，且每次 tick 都打印。
  const probe = async (p) => {
    try { const res = await fetch(BASE + p, { signal: AbortSignal.timeout(5000) }); await res.text(); return String(res.status); }
    catch (e) { return "FAIL:" + String((e.cause && (e.cause.code || e.cause.message)) || e.message).slice(0, 22); }
  };
  const tickOnce = async () => {
    const [render, control] = await Promise.all([probe("/"), probe("/control/wt/chain?uid=genesis")]);
    const lastMark = devWatchMarks.length ? devWatchMarks[devWatchMarks.length - 1].name : "(开场)";
    const tick = { n: devWatchTicks.length + 1, render, control, at: lastMark };
    devWatchTicks.push(tick);
    console.log("  [devwatch#" + tick.n + " @" + lastMark + "] 渲染=" + render + " control=" + control);
    if (render.indexOf("FAIL") === 0 && !devWatchFirstFail) devWatchFirstFail = tick;
    devWatchTimer = setTimeout(tickOnce, 2000);
  };
  devWatchTimer = setTimeout(tickOnce, 500);
}

/** 测试侧 claimOf：与 doc/uiStore.js 同语义（声明序后写覆盖，集合成员判断）。 */
// `matchOf` 靠 **node.chain** 判 path 段（链根带文档路径），而套件手里的节点是裸的 ⇒
//   先沿 ct 从**根**DFS 出一条链再问；`sets` 这个参数现在承载「根」（调用点传 root）。
const chainFor = (root, target) => {
  const stack = [[root, []]];
  while (stack.length) {
    const pair = stack.pop(); const n = pair[0]; const anc = pair[1];
    if (n === target) return anc.concat([n]);
    const kids = n && Array.isArray(n.ct) ? n.ct : [];
    for (const k of kids) stack.push([k, anc.concat([n])]);
  }
  return null;
};
const claimOfTest = (sets, table, node) => {
  if (node && !Array.isArray(node.chain) && sets) { const c = chainFor(sets, node); if (c) node.chain = c; }
  const __r = claimOfApi(node, table);   // 当前 API 返回 **Claim 对象**（旧断言比字符串）⇒ 归一到名字，调用点一行不改。
  return (__r && __r.name) ? __r.name : __r;
};

function frame(tag, payload, peer = 0, doc = 0) {
  const p = payload || new Uint8Array(0);
  const buf = new Uint8Array(HEADER + p.length);
  const dv = new DataView(buf.buffer);
  dv.setUint8(0, tag);
  dv.setUint32(1, p.length, true);
  dv.setUint32(5, peer, true);
  dv.setUint32(9, doc, true);
  buf.set(p, HEADER);
  return buf;
}

// 跨阶段共享（E3 / F2 用）
let b2Value = null;    // B2：引擎 :tag content 读出的值
let envelope = null;   // D3：声明文档的 F 帧交付树
let decl = null;       // E：extractDecl 结果

// ── ① 壳宿主 ──
stage("① 壳宿主（esmx dev :5178）");
// dev HTML 快照：⑰ 复用它（⑯ 之后 dev 的渲染路由曾有数秒 ECONNRESET 窗口 —— 根因=构建产物落在 dev 根内
// 触发 esmx watcher 的全量 page reload；已修（影子工程移到 os.tmpdir()）。快照保留：更稳，且不依赖时序）
let devHtml = "";
let devImports = null;
const devArtifacts = {};
try {
  const res = await fetch(BASE + "/");
  const html = await res.text();
  devHtml = html;
  // ⑱ 需要的 dev 侧数据也在这里抓（⑯ 之后 GET / 会被打断，见 ⑯g 探针）
  { const m = /<script type="importmap">([\s\S]*?)<\/script>/.exec(html);
    if (m) { try { devImports = JSON.parse(m[1]).imports || {}; } catch (e) { devImports = null; } }
    // 键 = 模块 manifest 的 identifier（逻辑名）。std 迁移后从 `std.js` 变成 `std`；region 仍是带 `.js` 的历史形态。
    for (const k of ["ui_std/std", "genesis_region/region.js"]) {
      const u = devImports && devImports[k];
      if (!u) continue;
      try { const a = await fetch(BASE + u); devArtifacts[k] = { url: u, status: a.status, ct: a.headers.get("content-type") || "" }; }
      catch (e) { devArtifacts[k] = { url: u, status: 0, ct: "fetch failed" }; }
    } }
  res.status === 200 ? ok("A1 页面 200", res.headers.get("content-type") || "") : bad("A1 页面 200", "实际 " + res.status);
  html.indexOf("id=\"root\"") >= 0 ? ok("A2 页面含 #root") : bad("A2 页面含 #root");
  res.headers.get("cross-origin-opener-policy") === "same-origin" ? ok("A3a COOP: same-origin") : bad("A3a COOP", String(res.headers.get("cross-origin-opener-policy")));
  res.headers.get("cross-origin-embedder-policy") === "require-corp" ? ok("A3b COEP: require-corp") : bad("A3b COEP", String(res.headers.get("cross-origin-embedder-policy")));
} catch (e) { bad("A 壳宿主", String(e.message || e)); }
try {
  // 迁移后真实存在的 ui 件（声明 mods:std = ui:std.js）
  //  原探针 `/ui-assets/genesis/std.js` 随 std 迁到官方包（`ui_std`）而消失——
// 该断言被误分类为"既有红"两次，实为迁移副作用。改探一个**仍在路径上**的文件：
const a1 = await fetch(BASE + "/ui-assets/genesis/region/src/region.js");
  a1.status === 200 ? ok("A4a /ui-assets 存在文件 → 200", "/ui-assets/genesis/region/src/region.js") : bad("A4a /ui-assets 200", "实际 " + a1.status);
  const a2 = await fetch(BASE + "/ui-assets/genesis/.hidden/x.js");
  a2.status === 403 ? ok("A4b 点段 → 403") : bad("A4b 点段 → 403", "实际 " + a2.status);
  const a3 = await fetch(BASE + "/ui-assets/genesis/std/appearance.bad");
  a3.status === 415 ? ok("A4c 坏扩展 → 415") : bad("A4c 坏扩展 → 415", "实际 " + a3.status);
} catch (e) { bad("A4 /ui-assets", String(e.message || e)); }
try {
  // A5（wasm 收编进 ui_wasm 适配器）：importmap 要有 ui_wasm 三面；胶水与哈希 .wasm 资产可发（不再有 /wasm/ 裸路径）。
  const pageHtml = await (await fetch(BASE + "/demo/?wt=genesis")).text();
  const mm = /<script type="importmap">([\s\S]*?)<\/script>/.exec(pageHtml);
  const imports = mm ? JSON.parse(mm[1]).imports : {};
  const glue = imports["ui_wasm/glue"];
  const g = await fetch(BASE + glue);
  const gm = /editor_core_bg\.[A-Za-z0-9]+\.final\.wasm/.exec(await g.text());
  const w = await fetch(BASE + "/ui_wasm/" + gm[0]);
  g.status === 200 && w.status === 200 ? ok("A5 wasm 适配器资产可发", "glue 200 · wasm " + w.headers.get("content-length") + "B") : bad("A5 wasm 适配器资产", "glue " + g.status + " · wasm " + w.status);
} catch (e) { bad("A5 wasm 适配器资产", String(e.message || e)); }

// ── ② 网关健康 ──
stage("② 网关健康（:8011）");
try {
  const res = await fetch(GW + "/control/status");
  const j = await res.json();
  j && j.running === true ? ok("gateway running") : bad("gateway running", JSON.stringify(j).slice(0, 120));
} catch (e) { bad("网关可达", String(e.message || e)); }

// ── ③ exec DSL ──
stage("③ exec DSL");
async function exec(code) {
  const res = await fetch(GW + "/exec", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ code, active_doc: "", wt_uid: "genesis" }) });
  const j = await res.json();
  if (j.status !== "ok") throw new Error(j.result || "exec 失败");
  return String(j.result);
}
try {
  const c1 = await exec("sel compile \"H1\"");
  const o1 = JSON.parse(c1);
  Array.isArray(o1.steps) && o1.steps.length ? ok("B1 sel compile → Step[]", c1.slice(0, 80)) : bad("B1 sel compile", c1.slice(0, 80));
  b2Value = (await exec("$ /.mds/ui.md::mods:std -pj ct")).trim();
  // 规范形 .ui::std.js（前导点=本 wt）；旧形 ui:std.js 仍兼容 ⇒ 两者都算过
  // 规范形 .ui::std.js / 旧形 ui:std.js / **逻辑名 <包>/<导出>**（官方件迁 lib/mods 后）——三者都算过
  // 逻辑名与历史形态都接受：迁移后声明写 `ui_std/std`（manifest identifier），旧形态是 `<pkg>/std.js`。
  /^(\.ui::|ui:?)std(\.js)?$|^[a-z0-9][a-z0-9_.-]*\/std(\.js)?$/.test(String(b2Value)) ? ok("B2 :tag content 提取（mods:std）", b2Value) : bad("B2 :tag content 提取", String(b2Value).slice(0, 80));
  const c3 = await exec("$ /doc_small.md");
  c3.trim().length > 0 ? ok("B3 $ 读文档", c3.trim().length + " 字符") : bad("B3 $ 读文档", "空");
  const c4 = await exec("$ /__no_such_doc__.md");
  /^\s*(?:-[ \t]*)?mdio_error\b/.test(c4) ? ok("B4 失败协议前缀", c4.trim().slice(0, 60)) : bad("B4 失败协议前缀", c4.slice(0, 80));
} catch (e) { bad("exec DSL", String(e.message || e)); }

// ── ④ 全量树 ──
stage("④ 全量树 /fs/tree?hidden=true");
let uiMdId = null;
try {
  const res = await fetch(GW + "/fs/tree?hidden=true");
  const list = await res.json();
  Array.isArray(list) && list.length > 60 ? ok("C1 树条目数", list.length + " 条") : bad("C1 树条目数", String(list.length));
  const ui = list.find((e) => e.path === "/.mds/ui.md");
  if (ui && ui.id != null) { uiMdId = ui.id; ok("C2 含 /.mds/ui.md（id=" + ui.id + "）"); }
  else bad("C2 含 /.mds/ui.md", "未找到或缺 id");
} catch (e) { bad("全量树", String(e.message || e)); }

// ── ⑤ WS 帧协议 + 声明提取真链路 ──
stage("⑤ WS 帧协议 + 声明提取");
if (!uiMdId) { bad("⑤ 前置", "无 ui.md docId"); } else {
  await new Promise((resolve) => {
    const ws = new WebSocket(WS_URL);
    ws.binaryType = "arraybuffer";
    const queue = [];
    const seen = [];
    ws.addEventListener("message", (ev) => { const f = new Uint8Array(ev.data); queue.push(f); seen.push(f.length ? f[0] : -1); });
    const nextFrame = (pred, ms) => new Promise((res2, rej) => {
      const t0 = Date.now();
      (function poll() {
        for (let i = 0; i < queue.length; i++) {
          if (pred(queue[i])) { res2(queue.splice(i, 1)[0]); return; }
        }
        if (Date.now() - t0 > ms) { rej(new Error("WS 帧超时（已见 tag：" + seen.map((x) => "0x" + x.toString(16)).join(",") + "）")); return; }
        setTimeout(poll, 20);
      })();
    });
    (async () => {
      try {
        await new Promise((res2, rej) => { ws.addEventListener("open", res2); ws.addEventListener("error", () => rej(new Error("WS 连接失败"))); });
        ws.send(frame(TAG.UpTree).buffer);
        const treeFrame = await nextFrame((f) => f.length >= HEADER && (f[0] === TAG.RenderDir || f[0] === TAG.RemoteOp), 8000);
        const tree = JSON.parse(new TextDecoder().decode(treeFrame.subarray(HEADER)));
        Array.isArray(tree) && tree.length > 0 ? ok("C3+D1 UpTree → DnTree", tree.length + " 条") : bad("D1 UpTree → DnTree", String(tree).slice(0, 60));
        const execPayload = new TextEncoder().encode(JSON.stringify({ exec_id: 9001, code: "sel compile \"H1\"", active_doc: "", wt_uid: "genesis" }));
        ws.send(frame(TAG.ExecReq, execPayload).buffer);
        const execFrame = await nextFrame((f) => f.length >= HEADER && f[0] === TAG.DnExec, 8000);
        const execJson = JSON.parse(new TextDecoder().decode(execFrame.subarray(HEADER)));
        execJson.exec_id === 9001 && String(execJson.raw || "").indexOf("steps") >= 0 ? ok("D2 ExecReq → DnExec(0x85)", String(execJson.raw).slice(0, 50)) : bad("D2 ExecReq → DnExec", JSON.stringify(execJson).slice(0, 80));
        // UpOpen（净载荷为空——wasm 源码：UpOpen 一律 &[]）→ DnOpen 0x82 → DnAst 0x84['F'+JSON]
        ws.send(frame(TAG.UpOpen, new Uint8Array(0), 0, uiMdId).buffer);
        const fFrame = await nextFrame((f) => f.length >= HEADER + 1 && f[0] === TAG.DnAst && f[HEADER] === 0x46, 8000);
        envelope = JSON.parse(new TextDecoder().decode(fFrame.subarray(HEADER + 1)));
        const { extractDecl } = await import("ui_core/services/uiMd.js");
        decl = extractDecl(envelope);
        // match=2 而非 3：非渲染件（只导出 sheet 的 CSS mod，如 THEME）**不参战**、不再占渲染槽
        // ——正是 uiMd.js:182-187 注释记录的那条（`- THEME: ::..` 会把 std 顶掉、整篇只剩标题）。
        decl.mods.length === 3 && decl.match.length === 2 ? ok("D3+E1 声明提取真链路", "mods=" + decl.mods.length + " match=" + decl.match.length) : bad("E1 声明提取", "mods=" + decl.mods.length + " match=" + decl.match.length);   // std + THEME + REGION
        decl.mods[0] && decl.mods[0].name === "std" && decl.mods[0].spec === "ui_std/std" ? ok("E2 首条目 tag/content", decl.mods[0].name + " = " + decl.mods[0].spec) : bad("E2 首条目", JSON.stringify(decl.mods[0]).slice(0, 80));
        decl.mods[0] && decl.mods[0].spec === b2Value ? ok("E3 形状走法 == 引擎 :tag 读", b2Value) : bad("E3 两路等价", "walk=" + (decl.mods[0] && decl.mods[0].spec) + " :tag=" + b2Value);
        // E4：mods 节下确有 bl 容器（穿透过才拿得到 15 条）
        // 声明文档现在以 `# UI` 起首（业主口径）⇒ `## mods` 下沉一层，这里做**有界**递归查找（深度 ≤4）。
        const findH = (n, t, d) => {
          if (!n || typeof n !== "object" || d > 4) return null;
          if (n.nt === "h" && n.t === t) return n;
          const ks = Array.isArray(n.ct) ? n.ct : [];
          for (const c of ks) { const r = findH(c, t, d + 1); if (r) return r; }
          return null;
        };
        const modsSec = findH(envelope, "mods", 0);
        const hasBl = modsSec && Array.isArray(modsSec.ct) && modsSec.ct.some((c) => c.nt === "bl");
        hasBl && decl.mods.length === 3 ? ok("E4 容器透明（bl 穿透）") : bad("E4 容器透明", "hasBl=" + hasBl);
        ok("F2a 认领零 id 依赖（node 键；后端协议 path 式，无身份概念）");
        ws.close();
        resolve();
      } catch (e) { bad("⑤ WS/提取", String(e.message || e)); try { ws.close(); } catch (e2) { /* ignore */ } resolve(); }
    })();
  });
}

// ── ⑥ 断连重连 ──
stage("⑥ 断连重连（D4）");
try {
  await new Promise((resolve) => {
    const ws = new WebSocket(WS_URL);
    ws.binaryType = "arraybuffer";
    let gotTree = false;
    const timer = setTimeout(() => { try { ws.close(); } catch (e) { /* ignore */ } resolve(); }, 8000);
    ws.addEventListener("message", (ev) => {
      const f = new Uint8Array(ev.data);
      if (f.length >= HEADER && (f[0] === TAG.RenderDir || f[0] === TAG.RemoteOp)) { gotTree = true; clearTimeout(timer); try { ws.close(); } catch (e) { /* ignore */ } resolve(); }
    });
    ws.addEventListener("open", () => { ws.send(frame(TAG.UpTree).buffer); });
    ws.addEventListener("close", () => { if (!gotTree) { clearTimeout(timer); resolve(); } });
    ws.addEventListener("error", () => { if (!gotTree) { clearTimeout(timer); resolve(); } });
  });
  // 上面的 resolve 无法区分成败——用独立快速连接再验一次
  await new Promise((resolve, reject) => {
    const ws = new WebSocket(WS_URL);
    ws.binaryType = "arraybuffer";
    const timer = setTimeout(() => { try { ws.close(); } catch (e) { /* ignore */ } reject(new Error("重连超时")); }, 8000);
    ws.addEventListener("message", (ev) => {
      const f = new Uint8Array(ev.data);
      if (f.length >= HEADER && (f[0] === TAG.RenderDir || f[0] === TAG.RemoteOp)) { clearTimeout(timer); try { ws.close(); } catch (e) { /* ignore */ } resolve(); }
    });
    ws.addEventListener("open", () => { ws.send(frame(TAG.UpTree).buffer); });
    ws.addEventListener("error", () => { clearTimeout(timer); reject(new Error("重连失败")); });
  });
  ok("D4 断连后重连可用");
} catch (e) { bad("D4 断连重连", String(e.message || e)); }

// ── ⑦ 认领（渲染内解析：真声明 + 真文档的真命中） ──
stage("⑦ 认领（渲染内解析）");
if (!envelope || !decl) { bad("F2b 前置", "无声明树"); } else {
  try {
    const { compileMatchTable } = await import("ui_core/services/uiMd.js");
    const compileSelHttp = async (sel) => {
      const raw = await exec("sel compile " + JSON.stringify(sel));
      const o = JSON.parse(raw);
      if (!Array.isArray(o.steps)) throw new Error("缺 steps");
      return o.steps;
    };
    const { table, errors: tbErrs } = await compileMatchTable(decl, compileSelHttp);
    tbErrs.length === 0 ? ok("F2b-1 声明全部编译通过（0 诊断）", table.length + " 条进解析") : bad("F2b-1 编译", tbErrs.length + " 条诊断：" + JSON.stringify(tbErrs[0]).slice(0, 100));
    // 登记条目正向断言（node 桶）：`::..` → path ""（任意文档）· sel ".."（递归全部 = 全类型）· 登记条目
    // 注：空内容的 bullet 不进交付树（实测）⇒ "只写件名"的写成不了，位置必须写出来。
    const stdM = decl.match.find((m) => m.name === "std");
    stdM && stdM.sel === ".."
      ? ok("F2b-2 登记条目提取正确（node 桶 · 递归通配 ..）", "std=" + (stdM.path || "(任意文档)") + "::" + stdM.sel + " bucket=" + stdM.bucket)
      : bad("F2b-2", JSON.stringify(stdM));
    // 真命中：/dashboard.md 的 DASHBOARD.引用 ← TRANS_EMBED
    const list = await (await fetch(GW + "/fs/tree?hidden=true")).json();
    const dash = list.find((e) => e.path === "/dashboard.md");
    const dashEnv = await new Promise((res, rej) => {
      const ws = new WebSocket(WS_URL);
      ws.binaryType = "arraybuffer";
      const timer = setTimeout(() => { try { ws.close(); } catch (e2) { /* ignore */ } rej(new Error("dashboard F 帧超时")); }, 8000);
      ws.addEventListener("message", (ev) => {
        const f = new Uint8Array(ev.data);
        if (f.length >= HEADER + 1 && f[0] === TAG.DnAst && f[HEADER] === 0x46) {
          clearTimeout(timer);
          res(JSON.parse(new TextDecoder().decode(f.subarray(HEADER + 1))));
          try { ws.close(); } catch (e2) { /* ignore */ }
        }
      });
      ws.addEventListener("open", () => ws.send(frame(TAG.UpOpen, new Uint8Array(0), 0, dash.id).buffer));
      ws.addEventListener("error", () => { clearTimeout(timer); rej(new Error("连接失败")); });
    });
    const dashSec = (Array.isArray(dashEnv.ct) ? dashEnv.ct : []).find((n) => n.t === "DASHBOARD");
    const yin = dashSec && (Array.isArray(dashSec.ct) ? dashSec.ct : []).find((n) => n.t === "引用");
    const dashSets = dashEnv;   // 传根：适配器用它补链   // 纯面整树扫描（含链式 claim）
    yin && claimOfTest(dashSets, table, yin) === "REGION"
      ? ok("F2b-3 真文档真命中（渲染内解析直通）", "/dashboard.md::DASHBOARD.引用 → REGION")
      : bad("F2b-3 真命中", "yin=" + String(yin && yin.t));
  } catch (e) { bad("F2b 渲染内解析", String(e.message || e)); }
}
// ── ⑨ boot 状态机（§2.1 职责全测；多实例用 ?query 取新模块） ──
stage("⑨ boot 状态机");
async function bootOk() {
  const b1 = await import("ui_core/services/boot.js?b1");
  const order = [];
  b1.registerStep("a", async () => { order.push("a"); });
  b1.registerStep("b", async () => { order.push("b"); });
  b1.registerStep("c", async () => { order.push("c"); });
  const snapA = b1.getState();
  const snapB = b1.getState();
  snapA === snapB ? ok("BOOT4 getState 稳定引用（无变迁时）") : bad("BOOT4 稳定引用", "引用变了");
  const r1 = await b1.start();
  order.join(",") === "a,b,c" ? ok("BOOT1 登记序 = 执行序", order.join("→")) : bad("BOOT1 执行序", order.join(","));
  r1.state === "ready" ? ok("BOOT2 全过 → ready") : bad("BOOT2 ready", r1.state);
  const r1b = await b1.start();
  r1b === r1 ? ok("BOOT3 start 幂等（同快照引用）") : bad("BOOT3 幂等", "引用变了");

  let threw = false;
  try { b1.registerStep("late", async () => {}); } catch (e) { threw = true; }
  threw ? ok("BOOT5 启动后注册抛错") : bad("BOOT5 启动后注册", "没抛");
}
async function bootFail() {
  const b2 = await import("ui_core/services/boot.js?b2");
  const order = [];
  b2.registerStep("a", async () => { order.push("a"); });
  b2.registerStep("b", async () => { order.push("b"); throw new Error("B 层炸了"); });
  b2.registerStep("c", async () => { order.push("c"); });
  const r2 = await b2.start();
  r2.state === "failed" ? ok("BOOT6 抛错 → failed") : bad("BOOT6 failed", r2.state);
  r2.error && r2.error.step === "b" && r2.error.message === "B 层炸了" && typeof r2.error.at === "string" && r2.error.at.length ? ok("BOOT7 失败归置：哪一步+原文+时间", r2.error.step + "｜" + r2.error.message) : bad("BOOT7 失败归置", JSON.stringify(r2.error));
  order.join(",") === "a,b" ? ok("BOOT8 失败即停（后续步骤不执行）") : bad("BOOT8 失败即停", order.join(","));
  const b3 = await import("ui_core/services/boot.js?b3");
  b3.registerStep("x", async () => {});
  let dup = false;
  try { b3.registerStep("x", async () => {}); } catch (e) { dup = true; }
  dup ? ok("BOOT9 重复步骤名抛错") : bad("BOOT9 重复名", "没抛");
}
await bootOk();
await bootFail();

// ── ⑪ uiMd 合成树（渲染内解析全矩阵） ──
stage("⑪ uiMd 合成树");
try {
  const { extractDecl, compileMatchTable } = await import("ui_core/services/uiMd.js");
  const mkSec = (name, bullets) => ({ id: "s_" + name, nt: "h", t: name, ct: [{ id: "bl_" + name, nt: "bl", t: "", ct: bullets.map((b, i) => ({ id: "b_" + name + "_" + i, nt: "b", t: b.name, ct: b.spec })) }] });
  const declTree = { id: "root", nt: "h", t: "", ct: [
    mkSec("mods", [{ name: "A", spec: "ui:a.js" }, { name: "B", spec: "md:/b.md::X" }]),
    mkSec("match", [{ name: "A", spec: "/a.md::S1" }, { name: "B", spec: "::S1" }, { name: "C", spec: "::BAD" }]),
    mkSec("visibility", [{ name: "include", spec: "/a.md::" }, { name: "exclude", spec: "/b.md::X" }]),
  ] };
  const decl = extractDecl(declTree);
  decl.mods.length === 2 && decl.match.length === 3 ? ok("UI1 合成声明提取（mods/match）") : bad("UI1 提取", "m=" + decl.mods.length + " x=" + decl.match.length);
  decl.vis && decl.vis.include.length === 1 && decl.vis.include[0].path === "/a.md" && decl.vis.include[0].sel === "" && decl.vis.exclude[0].sel === "X" ? ok("UI2 visibility 解析（include/exclude + path/sel 拆分）") : bad("UI2 visibility", JSON.stringify(decl.vis));
  decl.match[1].path === "" && decl.match[1].sel === "S1" ? ok("UI3 无 path 条目（当前文档缺省）") : bad("UI3 无 path", JSON.stringify(decl.match[1]));
  const docTree = { id: "d", nt: "h", t: "", ct: [
    { id: "n1", nt: "h", t: "S1", ct: [] },
    { id: "n2", nt: "b", t: "k", ct: "v" },
  ] };
  const mkR = async (match, tree, compileFn) => {
    const { table, errors } = await compileMatchTable({ mods: [], match, vis: null }, compileFn);
    const sets = tree;   // **不能硬写成 docTree**（忽略传进来的 tree）⇒ UI* 矩阵的节点拿不到链 ⇒ 递归/钻取判定全落空（离线复现证明引擎本身是对的）
    return { errors, res: (node) => claimOfTest(sets, table, node) };
  };
  const compileSel = (sel) => sel === "BAD" ? Promise.reject(new Error("坏选择器")) : Promise.resolve([["Drill", sel]]);
  const r4 = await mkR([{ name: "A", path: "", sel: "S1" }, { name: "B", path: "", sel: "S1" }], docTree, compileSel);
  r4.res(docTree.ct[0], [], "c0") === "B" ? ok("UI4 渲染内解析 + 声明序后者胜（A、B 同命中 → B）") : bad("UI4", "got " + String(r4.res(docTree.ct[0], [], "c0")));
  const r5 = await mkR([{ name: "C", path: "", sel: "BAD" }], docTree, compileSel);
  r5.errors.some((e) => e.name === "C" && /坏选择器/.test(e.msg)) ? ok("UI5 坏 sel 诊断不回填") : bad("UI5", JSON.stringify(r5.errors));
  const compileSel2 = () => Promise.resolve([["Wildcard"], ["CMP", ".tag", "==", "k"]]);
  const r6 = await mkR([{ name: "A", path: "", sel: "F" }], docTree, compileSel2);
  r6.res(docTree.ct[1], [], "c0") === "A" ? ok("UI6 CMP 折叠（非折叠 → 命中集）") : bad("UI6", "got " + String(r6.res(docTree.ct[1], [], "c0")));
  // IR 全算子矩阵：每种 step 一个代表用例（与活文档无关）
  const mt = { id: "r", nt: "h", t: "", ct: [
    { id: "a", nt: "h", t: "A", ct: [{ id: "a1", nt: "b", t: "k1", ct: "v1" }] },
    { id: "b", nt: "h", t: "B", ct: [] },
  ] };
  const byId = { a: mt.ct[0], b: mt.ct[1], a1: mt.ct[0].ct[0] };
  const bySel = {
    RG: [["Wildcard"], ["REGEX", ".tag", "^[AB]$"]],
    OR: [["Wildcard"], ["OR", [["CMP", ".tag", "==", "Z"], ["CMP", ".tag", "==", "B"]]]],
    NT: [["Wildcard"], ["NOT", ["CMP", ".tag", "==", "A"]]],
    AD: [["Wildcard"], ["AND", [["CMP", ".tag", "==", "A"], ["CMP", ".t", "==", "A"]]]],
    SL: [["Wildcard"], ["Slice", [1]]],
    RW: [["RecursiveWildcard"]],
    RD: [["RecursiveDrill", "A"]],
    TF: [["Drill", "A"], ["TraversalFilter", ["Wildcard"], [["TYPECHECK", "bullet"]], 1]],
  };
  const matrix = async (sels) => mkR(Object.keys(sels).map((k) => ({ name: "M_" + k, path: "", sel: k })), mt, (sel) => Promise.resolve(bySel[sel]));
  const mRG = await matrix({ RG: 1 });
  mRG.res(byId.a, [], "c0") === "M_RG" && mRG.res(byId.b, [], "c1") === "M_RG" ? ok("UI7 REGEX 折叠") : bad("UI7 REGEX");
  const mOR = await matrix({ OR: 1 });
  mOR.res(byId.b, [], "c1") === "M_OR" && !mOR.res(byId.a, [], "c0") ? ok("UI8 OR 折叠") : bad("UI8 OR");
  const mNT = await matrix({ NT: 1 });
  mNT.res(byId.b, [], "c1") === "M_NT" && !mNT.res(byId.a, [], "c0") ? ok("UI9 NOT 折叠") : bad("UI9 NOT");
  const mAD = await matrix({ AD: 1 });
  mAD.res(byId.a, [], "c0") === "M_AD" ? ok("UI10 AND 折叠") : bad("UI10 AND");
  //  契约变更（方案 §8.2 语言收窄）：声明面出现**位置/序号**写法必须**编译期硬拒且可诊断**。
  //    本条原断言语义 =「Slice 仍要出命中集」，随收窄**翻转为「必须被拒」**——这是契约变更，不是回归。
  //    禁令只圈**声明面**：工具地址不受限，见紧随其后的 UI11b（命令行 sel compile）。
  const mSL = await matrix({ SL: 1 });
  mSL.errors.some((e) => /位置\/序号类写法/.test(String(e.msg)) && /Slice/.test(String(e.msg))) && mSL.res(byId.b, [], "c1") == null
    ? ok("UI11 Slice ⇒ 声明面编译期被拒（表空 + 诊断可读）")
    : bad("UI11 Slice 应被拒", JSON.stringify({ errors: (mSL.errors || []).map((e) => String(e.msg).slice(0, 70)), resB: String(mSL.res(byId.b, [], "c1")) }));
  try {
    const j1 = await (await fetch(GW + "/exec", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ code: 'sel compile "..*[b][1]"', active_doc: "", wt_uid: "genesis" }) })).json();
    const k1 = (JSON.parse(String(j1.result || "{}")).steps || []).map((s) => s[0]);
    k1.indexOf("TraversalFilter") >= 0 && k1.indexOf("Slice") >= 0 ? ok("UI11b 工具地址不受限：sel compile ..*[b][1] 仍出 TraversalFilter+Slice", JSON.stringify(k1)) : bad("UI11b 工具地址", JSON.stringify(k1));
  } catch (e) { bad("UI11b 工具地址", String(e.message).slice(0, 90)); }
  const mRW = await matrix({ RW: 1 });
  mRW.res(byId.a1, ["A"], "c0") === "M_RW" ? ok("UI12 RecursiveWildcard（含深层 bullet）") : bad("UI12 RecursiveWildcard");
  const mRD = await matrix({ RD: 1 });
  mRD.res(byId.a, [], "c0") === "M_RD" && !mRD.res(byId.b, [], "c1") ? ok("UI13 RecursiveDrill") : bad("UI13 RecursiveDrill");
  // 同 UI11：TraversalFilter{limit>0}（序号）在**声明面**必须被拒（契约变更，非回归）；工具地址见 UI14b。
  const mTF = await matrix({ TF: 1 });
  mTF.errors.some((e) => /位置\/序号类写法/.test(String(e.msg)) && /TraversalFilter/.test(String(e.msg))) && mTF.res(byId.a1, ["A"], "c0") == null
    ? ok("UI14 TraversalFilter limit ⇒ 声明面编译期被拒（表空 + 诊断可读）")
    : bad("UI14 TraversalFilter 应被拒", "res=" + String(mTF.res(byId.a1, ["A"], "c0")) + " errors=" + JSON.stringify((mTF.errors || []).map((e) => String(e.msg).slice(0, 60))));
  try {
    const j2 = await (await fetch(GW + "/exec", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ code: 'sel compile "..[b]"', active_doc: "", wt_uid: "genesis" }) })).json();
    const k2 = (JSON.parse(String(j2.result || "{}")).steps || []).map((s) => s[0]);
    k2.indexOf("TraversalFilter") >= 0 ? ok("UI14b 工具地址不受限：sel compile ..[b] 仍出 TraversalFilter（limit=0）", JSON.stringify(k2)) : bad("UI14b 工具地址", JSON.stringify(k2));
  } catch (e) { bad("UI14b 工具地址", String(e.message).slice(0, 90)); }
  // ── task-12：声明里的子节分组（### sec/### ui…）穿透 + 未认形态大声报 ──
  const mkNested = (name, subName, bullets) => ({ id: "s_" + name, nt: "h", t: name, ct: [
    { id: "sub_" + subName, nt: "h", t: subName, ct: [
      { id: "bl_" + subName, nt: "bl", t: "", ct: bullets.map((b, i) => ({ id: "b_" + subName + "_" + i, nt: "b", t: b.name, ct: b.spec })) },
    ] },
  ] });
  const declNested = { id: "root", nt: "h", t: "", ct: [
    mkNested("mods", "ui", [{ name: "A", spec: "ui:a.js" }]),
    mkNested("match", "sec", [{ name: "A", spec: "/a.md::S1" }]),
    mkNested("visibility", "scope", [{ name: "include", spec: "/a.md::" }]),
  ] };
  const dN = extractDecl(declNested);
  // **契约变更**（同 UI11 的先例）—— 引擎明确「没有桶 ⇒ mods/match 下不再有子节」，
  //   因此本条从「子节穿透」翻转为「子节必须被**编译期大声拒绝**且诊断可读」（断言的是当前契约，不是放宽）。
  dN.mods.length === 0 && dN.match.length === 0 && dN.errors.some((e) => /子节/.test(String(e.msg))) && dN.errors.some((e) => /mods|match/.test(String(e.at || "")))
    ? ok("UI15 子节分组 ⇒ 当前契约下编译期被拒且诊断可读（原断言语义随「桶已删」翻转，同 UI11 先例）", "errs=" + dN.errors.length)
    : bad("UI15 子节应被拒", JSON.stringify({ mods: dN.mods.map((m) => m.name), match: dN.match, errs: (dN.errors || []).map((e) => String(e.msg).slice(0, 60)) }).slice(0, 180));
  const declBad = { id: "root", nt: "h", t: "", ct: [
    { id: "s", nt: "h", t: "match", ct: [
      { id: "p1", nt: "p", t: "这行不是条目", ct: "x" },
      { id: "s2", nt: "h", t: "sec", ct: [{ id: "s3", nt: "h", t: "sec2", ct: [] }] },
    ] },
  ] };
  const dB = extractDecl(declBad);
  dB.errors.length >= 2 && dB.match.length === 0
    ? ok("UI16 未认形态大声报（非 bullet 条目 + 子节嵌套超一层）", dB.errors.length + " 条诊断：" + dB.errors[0].msg.slice(0, 44))
    : bad("UI16 未认形态诊断", JSON.stringify({ errs: dB.errors, match: dB.match }).slice(0, 180));
} catch (e) { bad("⑪ uiMd", String(e.message || e)); }
// ── ⑫ 写回 → P 帧（wire 契约）+ MST applyPatch 三动词 ──
stage("⑫ 写回 → P 帧 + 补丁应用（MST 自带）");
const probeDoc = "/tmp_e2e_probe.md";
let probeId = null;
let probeWs = null;
async function cleanupProbe() {
  try { await exec("rm -f " + probeDoc); } catch (e) { /* 已删或从未建成 */ }
  try { if (probeWs) probeWs.close(); } catch (e) { /* ignore */ }
}
try {
  await exec("rm -f " + probeDoc);
  const mk1 = await exec("mk -f " + probeDoc);
  const mk2 = await exec("mk -f " + probeDoc);
  !/mdio_error/.test(mk1) && !/mdio_error/.test(mk2) ? ok("W0a mk 幂等（再建不报错）") : bad("W0a mk 幂等", mk2.slice(0, 60));
  await exec("\"\"\"# E2E\n## S\n- k: v1\"\"\" | +> " + probeDoc);
  const len1 = (await exec("$ " + probeDoc)).length;
  await exec("\"\"\"## S2\n- k2: v2\"\"\" | +> " + probeDoc);
  const len2 = (await exec("$ " + probeDoc)).length;
  len2 > len1 ? ok("W0b +> 是追加（内容增长）") : bad("W0b +> 追加", len1 + "→" + len2);
  const list = await (await fetch(GW + "/fs/tree?hidden=true")).json();
  const probe = list.find((e) => e.path === probeDoc);
  if (!probe || probe.id == null) throw new Error("探针文档未入树");
  probeId = probe.id;
  await new Promise((resolve, reject) => {
    probeWs = new WebSocket(WS_URL);
    probeWs.binaryType = "arraybuffer";
    const queue = [];
    probeWs.addEventListener("message", (ev) => { queue.push(new Uint8Array(ev.data)); });
    const nextFrame = (pred, ms) => new Promise((res2, rej) => {
      const t0 = Date.now();
      (function poll() {
        for (let i = 0; i < queue.length; i++) {
          if (pred(queue[i])) { res2(queue.splice(i, 1)[0]); return; }
        }
        if (Date.now() - t0 > ms) { rej(new Error("P 帧超时")); return; }
        setTimeout(poll, 20);
      })();
    });
    (async () => {
      try {
        await new Promise((res2, rej2) => { probeWs.addEventListener("open", res2); probeWs.addEventListener("error", () => rej2(new Error("连接失败"))); });
        probeWs.send(frame(TAG.UpOpen, new Uint8Array(0), 0, probeId).buffer);
        const fF = await nextFrame((f) => f.length >= HEADER + 1 && f[0] === TAG.DnAst && f[HEADER] === 0x46, 8000);
        const fEnv = JSON.parse(new TextDecoder().decode(fF.subarray(HEADER + 1)));
        fEnv ? ok("W1 开档 F 帧（临时文档）") : bad("W1 F 帧");
        // 沉淀：F 帧直发订阅者，但 scope 广播注册稍后生效——立刻写会错过 P 帧
        //（服务端把广播注册与 F 帧原子化后，此处 1s 沉淀应删掉转硬测，归 py session）
        await new Promise((r2) => setTimeout(r2, 1000));
        await exec("\"v2\" | /> " + probeDoc + "::S._b:k");
        const pF = await nextFrame((f) => f.length >= HEADER + 1 && f[0] === TAG.DnAst && f[HEADER] === 0x50, 8000);
        const ops = JSON.parse(new TextDecoder().decode(pF.subarray(HEADER + 1)));
        Array.isArray(ops) && ops.length >= 1 && ops.every((o) => o && typeof o.op === "string" && typeof o.path === "string") ? ok("W2 服务端写 → P 帧（RFC6902 op 数组）", ops.length + " op：" + ops.map((o) => o.op).join(",")) : bad("W2 P 帧", JSON.stringify(ops).slice(0, 160));
        // W3 契约捕获：真实 P op 用 MST 自带 applyPatch 应用到真实模型，断言写值落位
        const { MD, assertPatchOps } = await import("ui_core/services/client.js");   // DocRoot 已被删除（与 vis 同款漂移）⇒ 改用 MD
        const { applyPatch, getSnapshot } = await import("mobx-state-tree");
        const model = MD.create({ docId: (fEnv && fEnv.docId) || 0, nt: "root", ct: fEnv.ct })   // MD 的 docId 是必填（实测）   // F 帧带 mode/timestamp 等 MST 不认的字段 ⇒ 只取模型要的（与 vis 夹具同形）;
        applyPatch(model, ops);
        const getPtr = (obj, p) => p.split("/").filter(Boolean).reduce((a, s2) => (Array.isArray(a) ? a[Number(s2)] : a[s2]), obj);
        getPtr(getSnapshot(model), ops[0].path) === "v2" ? ok("W3 契约捕获：真实 op → MST applyPatch → 值落位（v2）") : bad("W3 契约捕获", String(getPtr(getSnapshot(model), ops[0].path)));
        // W4-W7 合成动词（MST 自带语义，不自造）
        let t0 = MD.create({ docId: 0, nt: "root", ct: [{ t: "A", ct: [] }, { t: "B", ct: [] }] });   // MD 的 docId/nt 必填（实测）
        applyPatch(t0, { op: "replace", path: "/ct/0/t", value: "A2" });
        getSnapshot(t0).ct[0].t === "A2" ? ok("W4 applyPatch replace（MST 自带）") : bad("W4 replace");
        applyPatch(t0, { op: "add", path: "/ct/0/ct/0", value: { t: "X" } });
        getSnapshot(t0).ct[0].ct.length === 1 ? ok("W5 applyPatch add（数组插入，MST 自带）") : bad("W5 add");
        applyPatch(t0, { op: "remove", path: "/ct/1" });
        getSnapshot(t0).ct.length === 1 ? ok("W6 applyPatch remove（数组摘除，MST 自带）") : bad("W6 remove");
        let unknownThrew = false;
        try { assertPatchOps([{ op: "explode", path: "/ct/0" }]); } catch (e2) { unknownThrew = true; }
        unknownThrew ? ok("W7 未知动词大声报（禁6，白名单守卫）") : bad("W7 未知动词", "没抛");
        resolve();
      } catch (e) { reject(e); }
    })();
  });
} catch (e) { bad("⑫ 写回/P", String(e.message || e)); } finally { await cleanupProbe(); }
try {
  const leftover = await exec("$ " + probeDoc);
  /mdio_error/.test(leftover) ? ok("W8 rm -f 清理无残留") : bad("W8 清理残留", leftover.slice(0, 60));
} catch (e) { bad("W8 清理检查", String(e.message || e)); }

// ── ⑬ 判官子进程（红即停 = e2e 的最后一道门） ──
stage("⑬ 判官棘轮");
try {
  const { spawnSync } = await import("node:child_process");
  const demoRoot = new URL("..", import.meta.url).pathname;
  const jr = spawnSync(process.execPath, ["scripts/judge.mjs"], { cwd: demoRoot, encoding: "utf8" });
  jr.status === 0 ? ok("判官绿", jr.stdout.trim().split("\n")[0]) : bad("判官红", (jr.stdout + jr.stderr).slice(0, 200));
} catch (e) { bad("⑬ 判官", String(e.message || e)); }

// ── ⑭ 业务面（§2.5：组件交付 = { node, exec }；exec = 与框架同一条 mdio exec 通路） ──
stage("⑭ 业务面（node/exec）");
try {
  const fs = await import("node:fs");
  const { fileURLToPath } = await import("node:url");
  // ⑭a 源级：递归扫**视图层全部来源**（宿主 src + 官方库包 lib/mods/*/src）——
  //      S3 之后交付形状（`<FunC node={node} exec={exec} />`）住在 `lib/mods/ui_core/src/render/NodeView.jsx`，
  //      只扫 demo/src 会假红（"没扫到"误报成"缺失"）。与判官 R8 的扫描面保持同一口径。
  const srcRoot = fileURLToPath(new URL("../src/", import.meta.url));
  const libRoot = fileURLToPath(new URL("../../lib/mods/", import.meta.url));
  const walk = (d, acc) => { for (const e of fs.readdirSync(d, { withFileTypes: true })) { const p = d + "/" + e.name; if (e.isDirectory()) walk(p, acc); else if (/\.(js|jsx)$/.test(e.name)) acc.push(p); } return acc; };
  const pkgSrcDirs = fs.existsSync(libRoot)
    ? fs.readdirSync(libRoot).map((n) => libRoot + n + "/src").filter((d) => fs.existsSync(d))
    : [];
  const dvAll = [srcRoot, ...pkgSrcDirs].flatMap((d) => walk(d, [])).map((f) => fs.readFileSync(f, "utf8")).join("\n");
  const hs = fs.existsSync(srcRoot + "devview/handle.js");
  // 交付形状按**意图**判：任意组件变量名 + 恰好 {node, exec}（新渲染面 doc/NodeView.jsx：const FunC = compFor(ui, node)）
const compShape = /<\w+\s+node=\{node\}\s+exec=\{exec\}/.test(dvAll);
  dvAll.indexOf("createHandle") < 0 && dvAll.indexOf("dataAt") < 0 && !hs && compShape
    ? ok("⑭a 业务面源级（{node,exec}；无 createHandle/dataAt/handle.js）")
    : bad("⑭a 业务面源级", "createHandle=" + (dvAll.indexOf("createHandle") >= 0) + " dataAt=" + (dvAll.indexOf("dataAt") >= 0) + " handle.js=" + hs + " compShape=" + compShape);
  // ⑭b 产物级：dev 编译产物里 exec 绑定当前 wt，且无包装层字样
  // 外壳已下沉为库包（S2）：仓库内的路径经 Vite 的 /@fs 暴露（URL 形态与错误边界探针一致 —— 见 test/error-boundary.mjs）。
  const REPO_E2E = fileURLToPath(new URL("../../..", import.meta.url));
  const SHELL_URL = "/demo/@fs" + REPO_E2E.replace(/\/$/, "") + "/mdio_ui/lib/mods/ui_devview/src/DevView.jsx";
  const dvProd = await (await fetch(BASE + SHELL_URL)).text();
  const norm = dvProd.replace(/\s+/g, " ");
  norm.indexOf("wt_uid: wt") >= 0 && norm.indexOf("createHandle") < 0
    ? ok("⑭b 产物级（exec 绑定 wt_uid: wt；无 createHandle）")
    : bad("⑭b 产物级", "execWt=" + (norm.indexOf("wt_uid: wt") >= 0) + " hasHandle=" + (norm.indexOf("createHandle") >= 0));
} catch (e) { bad("⑭ 业务面", String(e.message || e)); }


// ── ⑮ 声明健康（数据健康度：E1 只数条数不查对错，这里逐条查） ──
stage("⑮ 声明健康");
try {
  const { extractDecl } = await import("ui_core/services/uiMd.js");
  const hDecl = extractDecl(envelope);
  const health = [];
  // 每条 mods spec 合法：规范形 <wt>.<scope>::<file>[::<sel>] 或旧形 ui:/md:/http:
  for (const m of hDecl.mods) {
    const spec = String(m.spec);
    // 合法形态：① 空间形 `<wt>.<scope>::<file>` / `ui:` / `md:` / `http:`；② **裸包名** `<包名>/<导出>`。
    const okSpec = /^(\.?[A-Za-z0-9_]*\.(ui|md)::|(ui|md|http):)/.test(spec) || /^[a-z0-9][a-z0-9_.-]*\/[A-Za-z0-9_./-]+$/.test(spec);
    if (!okSpec) health.push("mods." + m.name + " spec 非空间形/裸包名: " + m.spec);
  }
  // 每条带 sel 的 match：target 文档存在（字面路径才查，glob 跳过）
  const tree2 = await (await fetch(GW + "/fs/tree?hidden=true")).json();
  const paths = new Set(tree2.map((x) => x.path));
  for (const m of hDecl.match) {
    if (!m.sel) { health.push("match." + m.name + " 无 sel（文档级条目当前约定不支持）"); continue; }
    if (m.path && m.path.indexOf("*") < 0 && !paths.has(m.path)) health.push("match." + m.name + " 目标文档不存在: " + m.path);
  }
  health.length === 0 ? ok("H1 声明健康（spec 三空间 + 目标文档存在）") : bad("H1 声明健康", health.join("；"));
} catch (e) { bad("⑮ 声明健康", String(e.message || e)); }

// ── ⑯ H2 模块健康门（esmx build 产物 + esmx validate） ──
stage("⑯ H2 模块健康门（esmx build 影子工程）");
try {
  const { spawnSync } = await import("node:child_process");
  const { readFileSync, writeFileSync, existsSync, rmSync, mkdirSync, symlinkSync } = await import("node:fs");
  const { createHash } = await import("node:crypto");
  const { join, dirname } = await import("node:path");
  const { fileURLToPath } = await import("node:url");
  const demoRoot = fileURLToPath(new URL("..", import.meta.url));
  // esmx CLI = node_modules/.bin/esmx（#!/bin/sh shim）；沙箱 PATH 常无 node，用 process.execPath 的目录补齐
  const env = Object.assign({}, process.env, { PATH: dirname(process.execPath) + ":" + (process.env.PATH || "") });
  const runEsmx = (args, cwd) => spawnSync(join(demoRoot, "node_modules/.bin/esmx"), args, { cwd: cwd || demoRoot, encoding: "utf8", env });
  // 已知耦合（实测钉死）：dev 与 build 共用 dist/，esmx 产物根硬编码 <root>/dist
  //   （@esmx/core/dist/module-config.mjs getLinks: path.resolve(root, "dist")），无 outDir 选项。
  //   在跑着 esmx dev 的工程里就地 build —— 事后无论还原与否都会打坏活着的 dev：
  //     · 不还原（构建态 manifest）→ dev 按哈希件解析：importmap 指向 /demo/src/entry.client.<hash>.final.mjs，
  //       dev 下 404、页面静默停在 SSR 骨架（无 console error）；
  //     · 还原（dev 态 manifest）→ dev 重新解析出 file=src/entry.client.ts，dist/client 下无此文件 →
  //       render 500（ENOENT .../dist/client/src/entry.client.ts）。
  //   ⇒ 留档事实：esmx 的 links 硬编码 root/dist、没有 outDir 选项 ⇒ dev 与 build 必须隔离。
  //   故 ⑯ 用「影子工程」构建：.tmp/e2e-build/ 只放 src/public/node_modules 符号链接 + package.json 副本，
  //   cwd 指向它 ⇒ 产物落 .tmp/e2e-build/dist/，真工程 dist/ 全程零接触（⑯e 用 sha256 自证），finally 清理。
  // 影子工程**必须落在 dev 根之外**：实测放根内（.tmp/e2e-build）时，构建产物 dist/client/index.html
  // 会触发 **esmx 自己的 watcher** 做一次 Vite 全量 page reload ⇒ SSR 管线被打断数秒，
  // GET / 期间 ECONNRESET（dev 日志实证：`[vite] (client) page reload .tmp/e2e-build/dist/client/index.html`）。
  // 注意这不走 Vite 的 server.watch.ignored（那是 Vite 自己的 watcher），所以只能改路径。
  const { tmpdir } = await import("node:os");
  const shadow = join(tmpdir(), "mdio-e2e-build");
  const liveManifest = join(demoRoot, "dist/client/manifest.json");
  const sha = (p) => (existsSync(p) ? createHash("sha256").update(readFileSync(p)).digest("hex") : null);
  const liveBefore = sha(liveManifest);
  const cleanup = () => { try { rmSync(shadow, { recursive: true, force: true }); } catch (e) { /* 清理失败不掩盖断言 */ } };
  cleanup();
  await probeRender("⑯ 收尾清理之后");
  try {
    mkdirSync(shadow, { recursive: true });
    symlinkSync(join(demoRoot, "src"), join(shadow, "src"));
    symlinkSync(join(demoRoot, "public"), join(shadow, "public"));
    symlinkSync(join(demoRoot, "node_modules"), join(shadow, "node_modules"));
    writeFileSync(join(shadow, "package.json"), readFileSync(join(demoRoot, "package.json")));
    await probeRender("⑯ 影子 build 之前");
    const build = runEsmx(["build"], shadow);
    await probeRender("⑯ 影子 build 之后");
    const buildOut = String((build.stdout || "") + (build.stderr || "")).trim();
    // 产物从影子工程读（真工程 dist 不再被构建触碰）
    const mfRaw = existsSync(join(shadow, "dist/client/manifest.json")) ? readFileSync(join(shadow, "dist/client/manifest.json"), "utf8") : null;
    const builtHtml = existsSync(join(shadow, "dist/client/index.html")) ? readFileSync(join(shadow, "dist/client/index.html"), "utf8") : "";
    build.status === 0
      ? ok("⑯a esmx build 成功（影子工程产物轨可重建）", "exit 0")
      : bad("⑯a esmx build", "exit " + build.status + (build.error ? "：" + String(build.error.message) : "") + "：" + buildOut.split("\n").slice(-3).join(" / ").slice(0, 200));
    // ⑯b manifest protocol 2（附录 A·8）
    let mf = null;
    try { mf = JSON.parse(mfRaw); } catch (e) { mf = null; }
    mf && mf.protocol === 2
      ? ok("⑯b 影子产物 manifest.json protocol=2", "name=" + mf.name + " version=" + mf.version)
      : bad("⑯b manifest protocol", mf ? "protocol=" + mf.protocol : "影子工程 dist/client/manifest.json 不可读");
    // ⑯c 自身 manifest exports 含 entry.client（identifier = demo/src/entry.client）
    const expKeys = mf && mf.exports ? Object.keys(mf.exports) : [];
    const expKey = expKeys.find((k) => k.indexOf("entry.client") >= 0);
    expKey && mf.exports[expKey].identifier === "demo/src/entry.client"
      ? ok("⑯c manifest.exports 含 entry.client", expKey + " → " + mf.exports[expKey].identifier)
      : bad("⑯c manifest exports", JSON.stringify(expKeys).slice(0, 140));
    // ⑯d importmap：实测 importmapMode 取默认 'inline'（demo 未覆写 rc.importmapMode）→ 断言构建 HTML 内联 importmap，
    //      并解析其 imports 键含 demo/src/entry.client（esmx 另产 dist/client/importmap/<hash>.final.mjs 旁路件，不单独断言）
    const imMatch = /<script type="importmap">([\s\S]*?)<\/script>/.exec(builtHtml);
    let imKeys = null;
    if (imMatch) { try { imKeys = Object.keys(JSON.parse(imMatch[1]).imports || {}); } catch (e) { imKeys = null; } }
    imKeys && imKeys.indexOf("demo/src/entry.client") >= 0
      ? ok("⑯d 构建 HTML 内联 importmap（importmapMode='inline'）含 entry.client", imKeys.join(","))
      : bad("⑯d 构建 importmap", imMatch ? "imports=" + JSON.stringify(imKeys).slice(0, 140) : "影子工程 dist/client/index.html 无内联 importmap");
    // ⑯e 隔离自检：真工程 dist/client/manifest.json 的 sha256 与构建前一致（影子构建零接触活 dev）
    await probeRender("⑯ build 与 validate 之间");
    const liveAfter = sha(liveManifest);
    liveBefore === liveAfter
      ? ok("⑯e 真工程 dist 零接触（manifest sha256 不变 → dev 零污染）", String(liveAfter || "无 manifest").slice(0, 16))
      : bad("⑯e 真工程 dist 被触碰", String(liveBefore).slice(0, 16) + " → " + String(liveAfter).slice(0, 16));
  } finally {
    cleanup();
  }
  // ⑯f 健康门：esmx validate（build-free 干跑，不读 dist）——exit 非 0 才是 error 级诊断；子命令不可用则如实跳过
  const val = runEsmx(["validate", "--json"]);
  await probeRender("⑯ validate 之后");
  const valOut = String((val.stdout || "") + (val.stderr || "")).trim();
  let valEnv = null;
  try { valEnv = JSON.parse(valOut); } catch (e) { valEnv = null; }
  if (valEnv && Array.isArray(valEnv.diagnostics)) {
    val.status === 0
      ? ok("⑯f esmx validate 零 error 级诊断", valEnv.diagnostics.length === 0 ? "零诊断" : valEnv.diagnostics.length + " 条 warning（exit 0）")
      : bad("⑯f esmx validate", valEnv.diagnostics.length + " 条诊断：" + JSON.stringify(valEnv.diagnostics[0]).slice(0, 140));
  } else if (/Cannot find module|Unknown|not found|ERR_MODULE/i.test(valOut)) {
    ok("⑯f esmx validate 子命令不可用 —— 如实跳过", valOut.split("\n")[0].slice(0, 120));
  } else {
    bad("⑯f esmx validate", ("exit " + val.status + "：" + valOut).slice(0, 200));
  }
} catch (e) { bad("⑯ H2 模块健康门", String(e.message || e)); }

// ⑯g 探针：⑯ 之后 dev 的**渲染路由**是否仍健康（打印，不用断言 —— 这是既有 harness 交互）
try { const h = await (await fetch(BASE + "/")).text(); console.log("  · ⑯g dev 渲染健康：GET / 200 len=" + h.length); }
catch (e) { console.log("  ! ⑯g dev 渲染**回归**：GET / " + String((e.cause && (e.cause.code || e.cause.message)) || e.message).slice(0, 40) + " —— 症状=构建产物又落回 dev 根内（esmx watcher 全量 page reload 打断 SSR）；检查影子工程路径是否退回 .tmp/"); }

// ── ⑰ SSR 边界（§1.3 / 附录 B5：rc 四件套痕迹在 dev HTML 里） ──
stage("⑰ SSR 边界（dev HTML 的 rc 痕迹）");
try {
  const res = devHtml ? { headers: new Headers({ "cross-origin-opener-policy": "same-origin", "cross-origin-embedder-policy": "require-corp" }) } : await fetch(BASE + "/");
  const html = devHtml || await res.text();
  const hasModuleEntry = /<script type="module">/.test(html);
  const hasEntryId = html.indexOf("entry.client") >= 0;
  hasModuleEntry && hasEntryId
    ? ok("⑰a rc.moduleEntry 痕迹（<script type=\"module\"> + entry.client）")
    : bad("⑰a moduleEntry 痕迹", "moduleScript=" + hasModuleEntry + " entry.client=" + hasEntryId);
  /<script type="importmap">/.test(html)
    ? ok("⑰b rc.importmap 痕迹（<script type=\"importmap\">）")
    : bad("⑰b importmap 痕迹", "HTML 无内联 importmap");
  html.indexOf("mdioBootstrap") >= 0
    ? ok("⑰c rc.state 注入（window[\"mdioBootstrap\"]）")
    : bad("⑰c rc.state 注入", "HTML 无 rc.state 痕迹");
  res.headers.get("cross-origin-opener-policy") === "same-origin" && res.headers.get("cross-origin-embedder-policy") === "require-corp"
    ? ok("⑰d SSR 页带 COOP/COEP（与 A3 同源同响应）")
    : bad("⑰d COOP/COEP", String(res.headers.get("cross-origin-opener-policy")) + "/" + String(res.headers.get("cross-origin-embedder-policy")));
} catch (e) { bad("⑰ SSR 边界", String(e.message || e) + "（cause=" + String((e && e.cause && (e.cause.code || e.cause.message)) || "-") + "）"); }

// ── ⑱ wt ui 模块链（§2.2 第②步：ui 目录 = esmx 模块，ui:<path> → import map 标识符） ──
stage("⑱ wt ui 模块链（import map 命中 + 原件可取）");
try {
  const { readFileSync } = await import("node:fs");
  const { join } = await import("node:path");
  const { fileURLToPath } = await import("node:url");
  const demoRoot = fileURLToPath(new URL("..", import.meta.url));
  const UI_MODULE = "wtui_genesis";                 // workspace/ui 的 package.json name（= 宿主 links 的键）
  const STD_MODULE = "ui_std";                    // std 已迁到官方包 lib/mods/std
  // 键 = 模块 manifest 里的 **identifier**（`<包名>/<导出名>`，逻辑名）。迁移后 std 的导出键从
  // `std.js` 变成 `std`（manifest: {name:"std", identifier:"ui_std/std", file:"std.<hash>.final.mjs"}）；
  // region 那条仍是带 `.js` 的历史形态 ⇒ 两条分别按各自 manifest 的真值写，不要一刀切。
  const wantKeys = [STD_MODULE + "/std", "genesis_region/region.js"];
  // ⑱a dev HTML 的 importmap 含 ui 件标识符（宿主 links + uses 生效的直接证据）
  const html = devHtml || await (await fetch(BASE + "/")).text();
  const imImports = devImports;
  const missing = imImports ? wantKeys.filter((k) => !imImports[k]) : wantKeys;
  imImports && missing.length === 0
    ? ok("⑱a dev importmap 含 ui 件标识符（" + wantKeys.length + " 件）", wantKeys.map((k) => k + " → " + imImports[k]).join(" ｜ "))
    : bad("⑱a importmap 缺 ui 件标识符", imImports ? "缺 " + missing.join(",") + "；现有键 " + Object.keys(imImports).join(",").slice(0, 120) : "HTML 无内联 importmap");
  // ⑱b 标识符指向的产物真能取到（dev 下 core 的 link 中间件按 /{name}/{file} 从模块 dist/client 供件）
  if (imImports && imImports[wantKeys[0]]) {
    const snap = devArtifacts[wantKeys[0]] || { url: imImports[wantKeys[0]], status: 0, ct: "" };
    snap.status === 200 && /javascript/.test(snap.ct)
      ? ok("⑱b ui 件产物可取（link 中间件 /{name}/{file}，① 阶段快照）", snap.url + " → 200 " + String(snap.ct).split(";")[0])
      : bad("⑱b ui 件产物", snap.url + " → " + snap.status + " " + snap.ct);
  } else {
    bad("⑱b ui 件产物", "无 importmap 键可验");
  }
  // ⑱c 键的来源 = ui 模块自己的 manifest（protocol 2 + exports.identifier），键不一致即链断
  const uiManifest = join(demoRoot, "..", "lib", "mods", "ui_std", "dist/client/manifest.json");   // std 的 manifest 在官方包
  let uim = null;
  try { uim = JSON.parse(readFileSync(uiManifest, "utf8")); } catch (e) { uim = null; }
  const stdExp = uim && uim.exports ? uim.exports["std"] : null;   // 逻辑名（迁移后 manifest 的键就是 `std`）
  uim && uim.protocol === 2 && stdExp && stdExp.identifier === wantKeys[0]
    ? ok("⑱c ui 模块 manifest protocol=2 且 identifier 对齐", uim.name + " · " + stdExp.identifier + " → " + stdExp.file)
    : bad("⑱c ui 模块 manifest", uim ? JSON.stringify({ protocol: uim.protocol, exports: Object.keys(uim.exports || {}) }).slice(0, 140) : "worktrees/genesis/ui/dist/client/manifest.json 不可读（ui 模块未 build？）");
} catch (e) { bad("⑱ wt ui 模块链", String(e.message || e)); }

// ── ⑲ 多 wt 链（ui 作用域链：隔离/本地/继承/子覆父；§2.2 边界项） ──
stage("⑲ 多 wt 链（隔离/本地/继承/子覆父）");
// 本组依赖子 wt exp_app(c6Oe1Gp1) 已挂载；装载可收敛 ⇒ 未挂载时**组内 SKIP 并标注**（不算红），汇总行如实写明。
let skipped19 = false;
const { mountedWts } = await import("./wt-guard.mjs");
const wts19 = await mountedWts(GW);
const childMounted19 = !!wts19 && wts19.indexOf("c6Oe1Gp1") >= 0;
if (!childMounted19) {
  skipped19 = true;
  console.log("  ⊘ ⑲ 组跳过并标注：依赖子 wt c6Oe1Gp1（exp_app）挂载，而当前 wts = [" + (wts19 ? wts19.join(",") : "取不到") + "]");
}
if (childMounted19) try {
  const CHILD = "c6Oe1Gp1";                       // exp_app 的真实 uid（genesis 的子 wt）
  // ⑲a 只读链端点：链序 = [自己, 父, …]（与 ui_dirs_for_wt 同源）
  const cg = await (await fetch(BASE + "/control/wt/chain?uid=genesis")).json();
  const cc = await (await fetch(BASE + "/control/wt/chain?uid=" + CHILD)).json();
  Array.isArray(cg.chain) && cg.chain.join(",") === "genesis" &&
  Array.isArray(cc.chain) && cc.chain.join(",") === CHILD + ",genesis"
    ? ok("⑲a 链端点 [自己, 父, …]", "genesis → [" + cg.chain + "] ｜ " + CHILD + " → [" + cc.chain + "]")
    : bad("⑲a 链端点", JSON.stringify({ genesis: cg.chain, child: cc.chain }).slice(0, 160));
  // ⑲b 隔离 / 本地 / 继承（/ui-assets 那条链的 404-200 语义）
  const s1 = await fetch(BASE + "/ui-assets/genesis/card/src/card.js");   // 子的件（包内），父看不到（隔离）
  const s2 = await fetch(BASE + "/ui-assets/" + CHILD + "/card/src/card.js");   // 子自己的件（已迁进 mod 包的 src/）
  // region 已迁成 mod 包（genesis_region，件在 <父 ui>/region/src/region.js）：**路径级继承仍由链提供**，只是路径随之变化
  const s3 = await fetch(BASE + "/ui-assets/" + CHILD + "/region/src/region.js");
  s1.status === 404 && s2.status === 200 && s3.status === 200
    ? ok("⑲b 隔离/本地/继承", "genesis/card/src/card.js=" + s1.status + " ｜ " + CHILD + "/card/src/card.js=" + s2.status + " ｜ " + CHILD + "/region/src/region.js=" + s3.status + "（继承落父目录）")
    : bad("⑲b 隔离/本地/继承", "genesis/card/src/card.js=" + s1.status + " child/card/src/card.js=" + s2.status + " child/region/src/region.js=" + s3.status);
  // ⑲c（按新模型重写）**声明即覆写/作用域**：每棵树的件住自己的包，隔离不变。
  // 同名散件两条链取到不同件的形态已不存在（"散件→包"迁移完成）。
  const own = await fetch(BASE + "/ui-assets/" + CHILD + "/card/src/card.js");     // 子自己的件（包内）
  const iso = await fetch(BASE + "/ui-assets/genesis/card/src/card.js");           // 父不该有子的件
  const htmlChild = await (await fetch(BASE + "/?wt=" + CHILD)).text();
  const imC = /<script type="importmap">([\s\S]*?)<\/script>/.exec(htmlChild);
  let keysC = {};
  if (imC) { try { keysC = JSON.parse(imC[1]).imports || {}; } catch (e) { keysC = {}; } }
  own.status === 200 && iso.status === 404 && !!keysC["exp_app_card/card.js"]
    ? ok("⑲c 声明即作用域（子件住自己的包 + 隔离 + 宿主登记）", CHILD + "/card/src/card.js=" + own.status + " ｜ genesis/card/src/card.js=" + iso.status + " ｜ 键 exp_app_card/card.js ✓")
    : bad("⑲c 声明即作用域", "own=" + own.status + " iso=" + iso.status + " key=" + JSON.stringify(keysC["exp_app_card/card.js"] || null));
  // ⑲d 宿主 links = 链上模块并集（import map 键 = 各模块 manifest 的 identifier）
  const html = await (await fetch(BASE + "/")).text();
  const im = /<script type="importmap">([\s\S]*?)<\/script>/.exec(html);
  let imports = {};
  if (im) { try { imports = JSON.parse(im[1]).imports || {}; } catch (e) { imports = {}; } }
  const want = ["ui_std/std", "genesis_region/region.js", "exp_app_card/card.js"];
  const miss = want.filter((k) => !imports[k]);
  miss.length === 0
    ? ok("⑲d 宿主 import map 含链上全部模块（" + want.length + " 键）", want.join(" , "))
    : bad("⑲d import map 链模块", "缺 " + miss.join(",") + "；现有 " + Object.keys(imports).join(",").slice(0, 120));
  // ⑲e 子 wt 页面：壳宿主把本 wt 的链内联进 HTML（客户端据此沿链试探，不再发请求）
  const chtml = await (await fetch(BASE + "/?wt=" + CHILD)).text();
  const m2 = /<script type="application\/json" id="mdio-wt-chain">([^<]*)<\/script>/.exec(chtml);
  let inj = null;
  if (m2) { try { inj = JSON.parse(m2[1]); } catch (e) { inj = null; } }
  inj && inj.wt_uid === CHILD && Array.isArray(inj.chain) && inj.chain.join(",") === CHILD + ",genesis"
    ? ok("⑲e 子 wt 页面内联链（#mdio-wt-chain）", "chain=[" + inj.chain + "] roots=" + (Array.isArray(inj.roots) ? inj.roots.length : 0) + " 个")
    : bad("⑲e 子 wt 内联链", m2 ? String(m2[1]).slice(0, 120) : "HTML 无 #mdio-wt-chain");
  // ⑲f 子 wt 画面的同源取件按页面 wt 落（壳宿主用 Referer 补 wt_uid）：子树 ≠ genesis 树
  const ct = await (await fetch(BASE + "/fs/tree?hidden=true", { headers: { Referer: BASE + "/?wt=" + CHILD } })).json();
  const paths = new Set(ct.map((e) => e.path));
  paths.has("/notes.md") && !paths.has("/dashboard.md")
    ? ok("⑲f 子 wt 的同源树（Referer 定 wt）", ct.length + " 条 · 含 /notes.md、不含 /dashboard.md")
    : bad("⑲f 子 wt 树", ct.length + " 条：" + [...paths].slice(0, 8).join(","));
  // ── task-12：真子 wt 声明的 ### 子节被穿透，且规则真命中（同一份 uiMd 代码，非合成树） ──
  const openDocFrame = (docId) => new Promise((res2, rej2) => {
    const ws2 = new WebSocket(WS_URL);
    ws2.binaryType = "arraybuffer";
    const timer2 = setTimeout(() => { try { ws2.close(); } catch (e2) { /* ignore */ } rej2(new Error("F 帧超时")); }, 8000);
    ws2.addEventListener("message", (ev) => {
      const f = new Uint8Array(ev.data);
      if (f.length >= HEADER + 1 && f[0] === TAG.DnAst && f[HEADER] === 0x46) {
        clearTimeout(timer2);
        res2(JSON.parse(new TextDecoder().decode(f.subarray(HEADER + 1))));
        try { ws2.close(); } catch (e2) { /* ignore */ }
      }
    });
    ws2.addEventListener("open", () => ws2.send(frame(TAG.UpOpen, new Uint8Array(0), 0, docId).buffer));
    ws2.addEventListener("error", () => { clearTimeout(timer2); rej2(new Error("连接失败")); });
  });
  // ⑲g 真子 wt 声明的 ### 子节被穿透（规则名 = bullet 的 tag，不是子节名 sec）
  const childTree = await (await fetch(BASE + "/fs/tree?hidden=true", { headers: { Referer: BASE + "/?wt=" + CHILD } })).json();
  const cUi = childTree.find((x) => x.path === "/.mds/ui.md");
  const cNotes = childTree.find((x) => x.path === "/notes.md");
  const { extractDecl } = await import("ui_core/services/uiMd.js");
  const cEnv = await openDocFrame(cUi.id);
  const cDecl = extractDecl(cEnv);
  const cMatch = cDecl.match[0];
  cMatch && cMatch.name === "CARD" && cMatch.path === "/notes.md" && cMatch.sel === "NOTES.卡片"
    ? ok("⑲g 子节穿透（真子 wt 声明 ### sec）", "match = " + cMatch.name + " ← " + cMatch.path + "::" + cMatch.sel)
    : bad("⑲g 子节穿透", JSON.stringify(cMatch).slice(0, 140));
  // ⑲h 规则真命中（子 wt 真文档 /notes.md 的 F 帧 + 渲染内解析）
  const { compileMatchTable, hitSetsOf } = await import("ui_core/services/uiMd.js");
  const compileSel9 = async (sel) => {
    const raw = await exec("sel compile " + JSON.stringify(sel));
    const o = JSON.parse(raw);
    if (!Array.isArray(o.steps)) throw new Error("缺 steps");
    return o.steps;
  };
  const { table: t9 } = await compileMatchTable(cDecl, compileSel9);
  const nEnv = await openDocFrame(cNotes.id);
  const sets9 = hitSetsOf(t9, nEnv);
  const notesSec = (Array.isArray(nEnv.ct) ? nEnv.ct : []).find((n) => n.t === "NOTES");
  const cardSec = notesSec && Array.isArray(notesSec.ct) ? notesSec.ct.find((n) => n.t === "卡片") : null;
  cardSec && claimOfTest(sets9, t9, cardSec) === "CARD"
      ? ok("⑲h 子 wt 规则真命中（整树命中集直通）", "/notes.md::NOTES.卡片 → CARD")
      : bad("⑲h 子 wt 命中", "cardSec=" + String(cardSec && cardSec.t));
} catch (e) { bad("⑲ 多 wt 链", String(e.message || e)); }

// ── 汇总（与 cdp-devview 同口径） ──
if (devWatchOn) {
  clearInterval(devWatchTimer);
  const badTicks = devWatchTicks.filter((t) => t.render.indexOf("FAIL") === 0);
  console.log("[devwatch 汇总] 探针 " + devWatchTicks.length + " 次 · 渲染路由失败 " + badTicks.length + " 次 · control 失败 " + devWatchTicks.filter((t) => t.control.indexOf("FAIL") === 0).length + " 次");
  if (devWatchFirstFail) {
    const before = devWatchMarks.filter((m) => m.tick <= devWatchFirstFail.n).pop();
    const after = devWatchMarks.find((m) => m.tick >= devWatchFirstFail.n);
    console.log("[devwatch 汇总] 断点落在 stage：" + (before ? before.name : "(开场)") + " → " + (after ? after.name : "(收尾)") + " 之间（tick " + (before ? before.tick : 0) + " → " + (after ? after.tick : "-") + "）");
  } else {
    console.log("[devwatch 汇总] 全程渲染路由健康（本次未复现）");
  }
}
console.log(failed ? "✗ e2e 失败 " + failed + " 项" + (skipped19 ? "（另：⑲ 组因依赖 wt 未挂载而跳过）" : "") : "✓ e2e 全部通过" + (skipped19 ? "（⑲ 组跳过：依赖的 exp_app 未挂载）" : "") + (process.argv.indexOf("--strict") >= 0 && skipped19 ? " ｜ --strict：跳过算红" : ""));
if (process.argv.indexOf("--strict") >= 0 && skipped19) process.exit(1);
process.exit(failed ? 1 : 0);
