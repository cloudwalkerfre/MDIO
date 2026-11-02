// @ts-check
/**
 * restart-host —— **重启 dev 宿主并等到真就绪**（消掉两类反复踩过的时序噪声）：
 *  ① 杀旧进程后**不立刻**起新进程，而是先等 5178 释放（否则新进程 bind 失败、`EXIT=1` 且输出为空，看着像无故退出）；
 *  ② 起来之后**轮询就绪**（对 `/demo/?wt=genesis` 取 200），而不是盲等固定秒数。
 *
 * 用法：node scripts/restart-host.mjs [--no-kill] [--check] [--log <path>]
 *   --check   只做就绪探测（不起新进程）
 *   --no-kill 不杀旧进程（若端口被占，会直接判定"已就绪"并退出）
 */
import { spawn, spawnSync, execSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

const DEMO = path.resolve(import.meta.dirname, "..");
const PORT = Number(process.env.PORT || 5178);
const URL_READY = "http://127.0.0.1:" + PORT + "/demo/?wt=genesis";
const args = process.argv.slice(2);
const noKill = args.includes("--no-kill");
const checkOnly = args.includes("--check");
const logIdx = args.indexOf("--log");
const LOG = logIdx >= 0 ? path.resolve(args[logIdx + 1]) : path.join(DEMO, ".tmp/host.log");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const pidsOnPort = () => {
  try {
    return execSync("lsof -nP -iTCP:" + PORT + " -sTCP:LISTEN -t", { encoding: "utf8" })
      .split(String.fromCharCode(10)).map((s) => s.trim()).filter(Boolean);
  } catch (e) { return []; }   // lsof 无输出时退出码非 0
};
/**
 * 就绪判据（加严）：
 *  ① HTTP 200 —— 不够：重启后第一次取页要付 Vite 预打包/预转换的代价，而且网关的 wt scope
 *     可能还没重新建立（实测：200 之后跑 live 套件曾整片红，gateway 日志留下 `remove_peer … Broken pipe`）。
 *  ② import map 存在且至少 5 条键 —— 证明宿主已把该 wt 的链内联进页面（声明面可用）。
 *  ③ **预热**：取一次页面 + 取 import map 里第一条模块 URL ⇒ 把 SSR/链内联与 Vite 预转换都先跑热。
 * 三条都过才算就绪；`verify-all`/`up-all` 因此不再把"冷页面"当成"可用页面"。
 */
const PROBE_KEYS = 5;
const pageReady = async () => {
  try {
    const r = await fetch(URL_READY, { signal: AbortSignal.timeout(4000) });
    if (r.status !== 200) return false;
    const html = await r.text();
    const m = /<script type="importmap">([\s\S]*?)<\/script>/.exec(html);
    if (!m) return false;
    let keys = [];
    try { keys = Object.keys((JSON.parse(m[1]).imports) || {}); } catch (e) { return false; }
    return keys.length >= PROBE_KEYS;
  } catch (e) { return false; }
};
const warmup = async () => {
  try {
    const r = await fetch(URL_READY, { signal: AbortSignal.timeout(6000) });
    const html = await r.text();
    const m = /<script type="importmap">([\s\S]*?)<\/script>/.exec(html);
    if (!m) return;
    const imports = JSON.parse(m[1]).imports || {};
    const first = Object.values(imports).find((u) => typeof u === "string" && u.startsWith("/"));
    if (first) await fetch(new URL(first, URL_READY), { signal: AbortSignal.timeout(6000) });
  } catch (e) { /* 预热失败不判死：下面的 ready 复检会兜住 */ }
};
const ready = async () => {
  if (!(await pageReady())) return false;
  return await pageReady();                       // 复检一次：预热过程中页面可能仍在变化
};
const waitReady = async (timeoutMs) => {
  const t0 = Date.now();
  while (Date.now() - t0 < timeoutMs) { if (await pageReady()) break; await sleep(500); }
  if (!(await pageReady())) return -1;
  await warmup();
  const ok = await ready();
  return ok ? Date.now() - t0 : -1;
};

if (checkOnly) {
  const ms = await waitReady(8000);
  console.log(ms >= 0 ? "✓ 宿主已就绪（" + ms + "ms）" : "✗ 宿主未就绪");
  process.exit(ms >= 0 ? 0 : 1);
}

if (noKill && await ready()) { console.log("✓ 宿主已在运行且就绪（--no-kill）"); process.exit(0); }

// **启动前预构建**：冷态下 `esmx dev` 会因 `uses` 模块缺 `dist` 而起不来，
// 所以先把"该建的"建好（旧宿主此时还在服务，不中断）—— `--no-preflight` 可跳过。
if (!args.includes("--no-preflight")) {
  // **先同步协议**：README 一直写"restart-host 已带预构建与同步"，实测只带了预构建 ✗。
  // 缺同步的后果：新登记/新构建的包不会进 `uses`/`link`/`node_modules` ⇒ 宿主起来后 `[E_NOT_LINKED]`。
  // 顺序：sync（列包）→ ensure-built（建未构建的）⇒ 与 `up-all` 的语义一致。
  const sy = spawnSync(process.execPath, ["scripts/sync-wt-uses.mjs"], { cwd: DEMO, encoding: "utf8" });
  if (sy.status !== 0) console.warn("  ⚠ 协议同步返回非零（exit=" + sy.status + "），继续尝试预构建");
  const pre = spawnSync(process.execPath, ["scripts/ensure-built.mjs"], { cwd: DEMO, encoding: "utf8" });
  const line = String(pre.stdout || "").split(String.fromCharCode(10)).filter((l) => l.trim()).pop() || "";
  console.log("· 预构建：" + line.trim());
  if (pre.status !== 0) console.warn("  ⚠ 预构建有失败项（宿主仍会启动，失败项会走 [autoBuild] 兜底）");
}

const before = pidsOnPort();
if (before.length) {
  console.log("· 结束占用 " + PORT + " 的进程: " + before.join(", "));
  for (const pid of before) { try { process.kill(Number(pid), "SIGTERM"); } catch (e) { /* 已退出 */ } }
}
let freed = false;
for (let i = 0; i < 40; i += 1) { if (pidsOnPort().length === 0) { freed = true; break; } await sleep(250); }
if (!freed) { console.error("✗ 端口 " + PORT + " 迟迟不释放；请手工检查"); process.exit(1); }
console.log("· 端口已释放 ✓");

fs.mkdirSync(path.dirname(LOG), { recursive: true });
const fd = fs.openSync(LOG, "a");
const child = spawn(path.join(DEMO, "node_modules/.bin/esmx"), ["dev"], {
  cwd: DEMO, detached: true, stdio: ["ignore", fd, fd],
});
child.unref();
console.log("· 已起新宿主 pid=" + child.pid + "（日志 " + path.relative(DEMO, LOG) + "）");

const ms = await waitReady(60000);
if (ms < 0) {
  console.error("✗ 60s 内未就绪；日志末尾：" + String.fromCharCode(10) + fs.readFileSync(LOG, "utf8").split(String.fromCharCode(10)).slice(-12).join(String.fromCharCode(10)));
  process.exit(1);
}
console.log("✓ 宿主就绪（" + ms + "ms）");
