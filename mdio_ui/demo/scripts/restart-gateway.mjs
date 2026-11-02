// @ts-check
/**
 * restart-gateway —— **安全重启引擎（网关）**：kill → 等端口释放 → 带**完整 env** 起 → 就绪 + 挂载根校验。
 *
 * 由来：用 `pkill` + 裸 `nohup python -m mdio_py.gateway` 重启过一次，
 * **漏了 env**（`MDIO_VFS_MOUNTS` / `MDIO_WORKSPACE_ROOT` 全丢）⇒ 页面起得来但声明装载不了，排查了半天。
 * 网关的 env 必须与 `up-all` 启动时**完全一致**（本文件从 up-all 抄的同一组，改了要两处一起改）。
 *
 * 用法：node scripts/restart-gateway.mjs [--check]
 *   默认：kill 现有进程 → 起新的 → 轮询 `/control/wt/chain`（200 且 `roots[0] === <挂载>/ui`）→ 退出码表示成功与否。
 *   `--check`：只看状态，不动进程（0=就绪且挂载匹配；1=未就绪/不匹配）。
 */
import { execSync, spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

const DEMO = path.resolve(import.meta.dirname, "..");
const REPO = path.resolve(DEMO, "../..");
const GW_PORT = Number(process.env.GW_PORT || 8011);
const MOUNT = process.env.MDIO_MOUNT || path.join(REPO, "worktrees/genesis");
const EXPECT_ROOT = path.join(MOUNT, "ui");          // chain 的 roots 是"该链的 ui 目录链"
const WT = process.env.WT || "genesis";
const checkOnly = process.argv.includes("--check");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const listeners = () => { try { return execSync("lsof -nP -iTCP:" + GW_PORT + " -sTCP:LISTEN -t", { encoding: "utf8" }).split(String.fromCharCode(10)).map((s) => s.trim()).filter(Boolean); } catch (e) { return []; } };

const state = async () => {
  try {
    const r = await fetch("http://127.0.0.1:" + GW_PORT + "/control/wt/chain?uid=" + WT, { signal: AbortSignal.timeout(2500) });
    if (r.status !== 200) return { ok: false, why: "HTTP " + r.status, roots: [] };
    const j = await r.json();
    const roots = Array.isArray(j && j.roots) ? j.roots : [];
    const ok = roots.length > 0 && path.resolve(String(roots[0])) === EXPECT_ROOT;
    return { ok, why: ok ? "就绪且挂载匹配" : "挂载不匹配（实际 " + JSON.stringify(roots) + "）", roots };
  } catch (e) { return { ok: false, why: "取不到（" + String(e.message).slice(0, 50) + "）", roots: [] }; }
};

if (checkOnly) {
  const st = await state();
  console.log("restart-gateway --check: " + (st.ok ? "✓ " : "✗ ") + st.why + "（pid " + listeners().join(",") + "）");
  process.exit(st.ok ? 0 : 1);
}

const before = listeners();
if (before.length) {
  console.log("· 结束占用 " + GW_PORT + " 的进程: " + before.join(","));
  for (const pid of before) { try { process.kill(Number(pid), "SIGTERM"); } catch (e) { /* 已退 */ } }
}
for (let i = 0; i < 20 && listeners().length; i += 1) await sleep(500);
if (listeners().length) { console.error("✗ 端口未释放：" + listeners().join(",")); process.exit(1); }
console.log("· 端口已释放 ✓");

const fd = fs.openSync(path.join(DEMO, ".tmp/gateway.log"), "a");
const child = spawn(path.join(REPO, ".venv/bin/python"), ["-u", "-m", "mdio_py.gateway"], {
  cwd: REPO, detached: true, stdio: ["ignore", fd, fd],
  env: Object.assign({}, process.env, {
    MDIO_WS_TRACE: "1", MDIO_GATEWAY_PORT: String(GW_PORT),
    MDIO_VFS_MOUNTS: MOUNT, MDIO_WORKSPACE_ROOT: MOUNT,
    MDIO_PROJ_MODE: process.env.MDIO_PROJ_MODE || "hs", MDIO_PATCH_GRANULARITY: process.env.MDIO_PATCH_GRANULARITY || "proj",
  }),
});
child.unref();
console.log("· 已起 pid=" + child.pid + "（日志 .tmp/gateway.log），等就绪…");

let ok = false; let why = "";
for (let i = 0; i < 40; i += 1) { const st = await state(); if (st.ok) { ok = true; why = st.why; break; } why = st.why; await sleep(500); }
console.log((ok ? "✓ " : "✗ ") + "网关" + (ok ? "就绪且挂载匹配" : "未就绪（20s）：" + why));
process.exit(ok ? 0 : 1);
