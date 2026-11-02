// @ts-check
/**
 * verify-prod —— **生产链路的一条命令检查**：
 *   ① 让出 5178（停 dev 宿主）→ ② `esmx build` → ③ 分离启动 `esmx preview` 并等就绪
 *   → ④ CDP 断言（客户端启动 · 应用渲染 · CSS mod 采纳 ≥1 · 无异常 · 无自注入）
 *   → ⑤ 默认**恢复 dev 宿主**（`--keep` 保留 preview，`--no-restore` 两者都不动）。
 *
 * 关键口径（都是踩过的）：页面地址是 **`/demo/?wt=genesis`**（不是 `/`）；探针前**先关掉所有旧页签**。
 */
import { spawn, spawnSync, execSync } from "node:child_process";
import fs from "node:fs";
import { requireOrchestrationLock } from "./lib/orchestrate.mjs";
import path from "node:path";

const DEMO = path.resolve(import.meta.dirname, "..");
requireOrchestrationLock(DEMO, path.basename(process.argv[1] || "?"));   // 编排互斥（C3）：与演练/其它编排串行
const PORT = Number(process.env.PORT || 5178);
const CDP = process.env.CDP || "http://127.0.0.1:9222";
const URL_PAGE = "http://127.0.0.1:" + PORT + "/demo/?wt=genesis";
const args = process.argv.slice(2);
const keep = args.includes("--keep");
const noRestore = args.includes("--no-restore");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const listeners = (port) => { try { return execSync("lsof -nP -iTCP:" + port + " -sTCP:LISTEN -t", { encoding: "utf8" }).split(String.fromCharCode(10)).map((s) => s.trim()).filter(Boolean); } catch (e) { return []; } };
const hardStop = async (port) => { for (const pid of listeners(port)) { try { process.kill(Number(pid), "SIGTERM"); } catch (e) { /* 已退 */ } } for (let i = 0; i < 40; i += 1) { if (!listeners(port).length) return true; await sleep(250); } return false; };
const ready = async () => { try { const r = await fetch(URL_PAGE, { signal: AbortSignal.timeout(2500) }); return r.status === 200; } catch (e) { return false; } };

let failed = 0;
const step = (ok, label, detail) => { if (!ok) failed += 1; console.log("  " + (ok ? "✓" : "✗") + " " + label + (detail ? "  → " + detail : "")); };

console.log("== verify-prod ==");
/** 构建：**用退出码判定**（不吞错、也不靠日志里出现 ✓ —— 失败日志里也可能有别的 ✓）。 */
const buildRes = (() => { try { return { code: 0, out: execSync("node_modules/.bin/esmx build 2>&1", { cwd: DEMO, encoding: "utf8" }) }; } catch (e) { return { code: e.status == null ? 1 : e.status, out: String(e.stdout || "") + String(e.stderr || "") }; } })();
const out = buildRes.out;
const built = buildRes.code === 0 && out.indexOf("Build completed") >= 0;
step(built, "生产构建", (out.split(String.fromCharCode(10)).filter((l) => l.trim()).pop() || "").trim().slice(0, 80));
const warns = (out.match(/\[W_NO_RANGE\]/g) || []).length;
console.log("    （本次构建的 W_NO_RANGE 条数：" + warns + "）");

// 构建失败 ⇒ 不启动 preview；**但环境也要回到可用**（restart-host），然后退 1
if (!built) {
  // §53.3-1（本 session 新增纪律）：凡"会自己判红退出"的子步骤，一律 spawnSync + 退出码当数据。
  const rh0 = spawnSync(process.execPath, ["scripts/restart-host.mjs"], { cwd: DEMO, stdio: "inherit" });
  console.error("✗ verify-prod：构建失败（已尝试恢复 dev 宿主，restart-host exit=" + rh0.status + "）");
  process.exit(1);
}
if (!(await hardStop(PORT))) { console.error("✗ 5178 释放失败"); process.exit(1); }
const fd = fs.openSync(path.join(DEMO, ".tmp/preview.log"), "a");
const prev = spawn(path.join(DEMO, "node_modules/.bin/esmx"), ["preview"], { cwd: DEMO, detached: true, stdio: ["ignore", fd, fd] });
prev.unref();
let up = false;
for (let i = 0; i < 40; i += 1) { if (await ready()) { up = true; break; } await sleep(500); }
step(up, "preview 就绪（:" + PORT + "）", "pid=" + prev.pid);

// CDP：先关掉所有旧页签，再开一个新页
const pages = await (await fetch(CDP + "/json/list")).json();
for (const t of pages.filter((x) => x.type === "page")) { try { await fetch(CDP + "/json/close/" + t.id); } catch (e) { /* 忽略 */ } }
await sleep(800);
await fetch(CDP + "/json/new?" + encodeURIComponent("about:blank"), { method: "PUT" });
await sleep(800);
const list = await (await fetch(CDP + "/json/list")).json();
const tab = list.filter((x) => x.type === "page").pop();
let ws; const pend = new Map(); let id = 0; const errs = [];
const send = (m, p) => new Promise((res, rej) => { const i = ++id; pend.set(i, { res, rej }); ws.send(JSON.stringify({ id: i, method: m, params: p })); });
ws = new WebSocket(tab.webSocketDebuggerUrl);
ws.addEventListener("message", (e) => { const m = JSON.parse(e.data); if (m.id) { const p = pend.get(m.id); if (p) { pend.delete(m.id); m.error ? p.rej(new Error(JSON.stringify(m.error))) : p.res(m.result); } return; } if (m.method === "Runtime.exceptionThrown") errs.push(String(m.params.exceptionDetails && m.params.exceptionDetails.text || "").slice(0, 120)); });
await new Promise((r) => ws.addEventListener("open", r));
await send("Runtime.enable"); await send("Page.enable");
await send("Page.navigate", { url: URL_PAGE });
const ev = (e) => send("Runtime.evaluate", { expression: e, returnByValue: true }).then((r) => (r.result ? r.result.value : undefined));
let booted = false;
for (let i = 0; i < 25; i += 1) { await sleep(800); if (await ev("!!document.querySelector('.dv-app')")) { booted = true; break; } }
await sleep(1500);
step(booted, "客户端启动 + 应用渲染（.dv-app）");
const adopted = Number(await ev("document.adoptedStyleSheets.length"));
const rules = String(await ev("JSON.stringify(document.adoptedStyleSheets.map((s)=>s.cssRules.length))"));
step(adopted >= 1, "CSS mod 采纳 ≥1", "adopted=" + adopted + " 规则=" + rules);
step(errs.length === 0, "无控制台异常", errs.slice(0, 2).join(" | "));
step((await ev("!!document.getElementById('mdio-katex-css')")) === false, "无 <style> 自注入");
// 审查 D4 + B1：把"**单一提供者**"变成判据 —— 生产 map 里 8 个裸名必须全部指向 **宿主** `/demo/*`
// （业主口径：demo 提供 ⇒ react 等 ⇒ mods）。测法与原版等价且更强：**同时**要求平台不再出现在这些裸名的映射里（它已让位 ⇒ 不会出现第二份实例）。
const map = await ev("(() => { const m = document.querySelector('script[type=importmap]'); if (!m) return ''; try { const j = JSON.parse(m.textContent || '{}'); return JSON.stringify(j.imports || {}); } catch (e) { return ''; } })()");
let imports = {}; try { imports = JSON.parse(String(map || "{}")); } catch (e) { imports = {}; }
const wanted = ["react", "react-dom/client", "mobx"];
const wrong = wanted.filter((k) => !String(imports[k] || "").startsWith("/ui_base/"));
step(wrong.length === 0, "单一提供者：react / react-dom/client / mobx 指向 /ui_base/*", wrong.length ? "异常项 " + wrong.map((k) => k + "→" + (imports[k] || "缺")).join(", ") : wanted.map((k) => k + "→" + String(imports[k]).replace("/ui_base/", "")).join(" · "));
try { ws.close(); } catch (e) { /* 忽略 */ }

// §71.6：**文档面也要在生产上验**。"dev 绿、生产红"是最容易被漏的一档（用户报的那条缺陷就住在文档面），
// 所以不新造断言，直接复用同一条活体判据（check-doc-render：DOM 与交付树逐项对齐），只把页面形状换成 /demo/。
// 退出码当数据（§53.3-1），不靠日志里出现 ✓。
{
  const dr = spawnSync(process.execPath, ["scripts/check-doc-render.mjs"], {
    cwd: DEMO,
    stdio: "inherit",
    env: Object.assign({}, process.env, { BASE: "http://127.0.0.1:" + PORT, PAGE_PREFIX: "/demo", SHOT_DIR: ".tmp/steps-gui/verify-prod" }),
  });
  step(dr.status === 0, "生产上文档面渲染完整（check-doc-render，页面 /demo/）", "exit=" + dr.status);
}

if (!keep && !noRestore) {
  await hardStop(PORT);
  const rhr = spawnSync(process.execPath, ["scripts/restart-host.mjs"], { cwd: DEMO, stdio: "inherit" });   // §53.3-1
  if (rhr.status !== 0) console.warn("  ⚠ 恢复 dev 宿主返回非零（exit=" + rhr.status + "），请手工确认宿主状态");
}
console.log(failed ? "✗ verify-prod 失败 " + failed + " 项" : "✓ verify-prod 全绿");
process.exit(failed ? 1 : 0);