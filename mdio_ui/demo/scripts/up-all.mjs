// @ts-check
/**
 * up-all —— **一条命令把整套拉起来**（操作序列脚本化）：
 *  ① `sync-wt-uses.mjs`（一处生成协议）；
 *  ② 网关：**已在监听就复用**，否则分离启动并等就绪；
 *  ③ 宿主：调 `restart-host.mjs`（它负责"等端口释放 + 就绪轮询"）；
 *  ④ 就绪检查：页面 200 + import map 三键在；
 *  ⑤ 打印判据入口。
 *
 * 用法：node scripts/up-all.mjs [--check]      （--check 只体检，不改动任何进程）
 */
import { spawn, spawnSync, execSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

const DEMO = path.resolve(import.meta.dirname, "..");
const REPO = path.resolve(DEMO, "../..");
const GW_PORT = Number(process.env.GW_PORT || 8011);
const HOST_PORT = Number(process.env.PORT || 5178);
const WT = process.env.WT || "genesis";
const checkOnly = process.argv.includes("--check");
const LOGDIR = path.join(DEMO, ".tmp");
/** 挂载根：网关与宿主共享的同一份「挂了哪棵树」事实（restart-gateway 同口径；env 可覆盖）。 */
const MOUNT = process.env.MDIO_MOUNT || path.join(REPO, "worktrees/genesis");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const listeners = (port) => { try { return execSync("lsof -nP -iTCP:" + port + " -sTCP:LISTEN -t", { encoding: "utf8" }).split(String.fromCharCode(10)).map((s) => s.trim()).filter(Boolean); } catch (e) { return []; } };

/** 网关就绪（审查 C8）：**还要核对挂载根** —— 只探 200 会把"另一个会话起的、挂了别的工作树的网关"当成可用。
 *  返回 { ok, roots }：ok 同时要求 200 且 roots[0] === 期望的 workspace 根。 */
// 注意口径：`/control/wt/chain` 的 `roots` 是**该链的 ui 目录链**（`ui_dirs_for_wt` 的产物），不是 workspace 根。
// 所以期望值是 `<挂载根>/ui`（网关以 MDIO_VFS_MOUNTS=<挂载根> 启动时）。
const EXPECT_ROOT = path.join(MOUNT, "ui");
const gwState = async () => {
  try {
    const r = await fetch("http://127.0.0.1:" + GW_PORT + "/control/wt/chain?uid=" + WT, { signal: AbortSignal.timeout(2500) });
    if (r.status !== 200) return { ok: false, roots: [] };
    const j = await r.json();
    const roots = Array.isArray(j && j.roots) ? j.roots : [];
    return { ok: roots.length > 0 && path.resolve(String(roots[0])) === EXPECT_ROOT, roots };
  } catch (e) { return { ok: false, roots: [] }; }
};
const gwReady = async () => (await gwState()).ok;
const pageState = async () => {
  try {
    const r = await fetch("http://127.0.0.1:" + HOST_PORT + "/demo/?wt=" + WT, { signal: AbortSignal.timeout(4000) });
    if (r.status !== 200) return { status: r.status, keys: [] };
    const html = await r.text();
    const keys = ["ui_std/std", "genesis_region/region", "exp_swap_swap/swap-a"].filter((k) => html.indexOf(k) >= 0);
    return { status: 200, keys };
  } catch (e) { return { status: 0, keys: [] }; }
};

console.log("== up-all ==");
console.log("· ① 协议同步（sync-wt-uses）");
if (!checkOnly) {
  // 同样按 §53.3-1：退出码当**数据**（sync 会因协议异常自己退非零 ⇒ 不能让它把 up-all 抛崩）。
  const sy = spawnSync(process.execPath, ["scripts/sync-wt-uses.mjs"], { cwd: DEMO, stdio: "inherit" });
  if (sy.status !== 0) console.error("  ⚠ 协议同步返回非零（exit=" + sy.status + "），继续尝试后续步骤");
}

console.log("· ①b 树的依赖落点（ensure-tree-deps：补 mdio_ui 链接 + 报告缺的运行时依赖）");
if (!checkOnly) {
  const td = spawnSync(process.execPath, ["scripts/ensure-tree-deps.mjs"], { cwd: DEMO, encoding: "utf8" });
  for (const line of String(td.stdout || "").split(String.fromCharCode(10))) if (line.trim()) console.log("  " + line.trim());
} else {
  const td = spawnSync(process.execPath, ["scripts/ensure-tree-deps.mjs", "--check"], { cwd: DEMO, encoding: "utf8" });
  for (const line of String(td.stdout || "").split(String.fromCharCode(10))) if (line.trim()) console.log("  " + line.trim());
}

console.log("· ② 网关（:" + GW_PORT + "）");
const gw = await gwState();
const gwListening = listeners(GW_PORT).length > 0;
if (gw.ok) {
  console.log("  ✓ 已在运行、就绪且**挂载根匹配**（复用；pid " + listeners(GW_PORT).join(",") + "）");
} else if (gwListening) {
  console.error("  ✗ " + GW_PORT + " 已被占用，但挂载根不匹配（期望 " + EXPECT_ROOT + "；实际 " + JSON.stringify(gw.roots) + "）");
  console.error("    ⇒ 拒绝复用它，也**不**另起一个（会 bind 失败）。请先停掉那个网关，或改 GW_PORT/环境后再跑。");
  process.exit(1);
} else if (checkOnly) {
  console.log("  ✗ 未就绪（--check 不启动）");
} else {
  fs.mkdirSync(LOGDIR, { recursive: true });
  const fd = fs.openSync(path.join(LOGDIR, "gateway.log"), "a");
  // **挂载事实必须同时给宿主**（修）：网关与宿主是**两个进程**，而"网关挂了哪棵树"这件事
  // 客户端要在连 WS 之前就知道（否则切到"在册未挂载"的树会卡在装载态）。宿主把它注入页面（injectHostMount）。
  const g = spawn(path.join(REPO, ".venv/bin/python"), ["-m", "mdio_py.gateway"], {
    cwd: REPO, detached: true, stdio: ["ignore", fd, fd],
    env: Object.assign({}, process.env, {
      MDIO_WS_TRACE: "1", MDIO_GATEWAY_PORT: String(GW_PORT),
      MDIO_VFS_MOUNTS: MOUNT,
      MDIO_WORKSPACE_ROOT: MOUNT,
      MDIO_PROJ_MODE: process.env.MDIO_PROJ_MODE || "hs", MDIO_PATCH_GRANULARITY: process.env.MDIO_PATCH_GRANULARITY || "proj",
    }),
  });
  g.unref();
  console.log("  已起 pid=" + g.pid + "（日志 .tmp/gateway.log），等就绪…");
  let ok = false;
  for (let i = 0; i < 40; i += 1) { if (await gwReady()) { ok = true; break; } await sleep(500); }
  console.log(ok ? "  ✓ 网关就绪" : "  ✗ 网关 20s 内未就绪（看 .tmp/gateway.log）");
}

console.log("· ③ 宿主（:" + HOST_PORT + "）");
if (checkOnly) {
  const st = await pageState();
  console.log("  " + (st.status === 200 ? "✓ 已在服务" : "✗ 未服务（HTTP " + st.status + "）"));
} else {
  // **不许抛异常**（§41.3-2 的纪律，up-all 自己也没遵守）：`restart-host` 在"宿主起不来"时退非零，
  // 用 execSync 会把整个 up-all 崩掉、连后面的诊断都看不到。用 spawnSync：退出码当**数据**，
  // 失败时把宿主日志尾部打出来（这正是本次真实故障的定位方式：`[E_NOT_LINKED]`）。
  // 宿主必须拿到同一份挂载事实（否则它注入给页面的值是空的 ⇒ 客户端无法在 WS 之前判死）。
  const rh = spawnSync(process.execPath, ["scripts/restart-host.mjs"], {
    cwd: DEMO, encoding: "utf8",
    env: Object.assign({}, process.env, { MDIO_VFS_MOUNTS: MOUNT }),
  });
  if (rh.stdout) process.stdout.write(rh.stdout);
  if (rh.status !== 0) {
    console.error("  ✗ 宿主启动失败（restart-host exit=" + rh.status + "）—— 宿主日志尾部：");
    try {
      const tail = execSync("tail -6 .tmp/host.log", { cwd: DEMO, encoding: "utf8" });
      for (const line of tail.split(String.fromCharCode(10))) if (line.trim()) console.error("    " + line.trim().slice(0, 160));
    } catch (e) { /* 日志取不到就算了 */ }
  }
}

console.log("· ④ 就绪检查");
const st = await pageState();
console.log("  页面 /demo/?wt=" + WT + " → HTTP " + st.status);
console.log("  import map 三键：" + (st.keys.length === 3 ? "✓ 全在" : "缺 " + (3 - st.keys.length) + " 个 → " + JSON.stringify(st.keys)));

console.log("· ⑤ 待建包清单（只读；同一份 packageState 判据）");
try {
  const dry = execSync("node scripts/ensure-built.mjs --dry", { cwd: DEMO, encoding: "utf8" }).trim();
  console.log("  " + dry);
} catch (e) { console.log("  ⚠ 取不到（" + String(e.message).slice(0, 60) + "）"); }

console.log("· ⑥ 判据入口（在 mdio_ui/demo 下跑）");
for (const c of ["node scripts/judge.mjs", "node scripts/check-layers.mjs", "SHOT_DIR=.tmp/steps-gui/x node test/pages-live.mjs", "node test/swap-live.mjs", "node test/e2e.mjs"]) console.log("    " + c);
process.exit(st.status === 200 && st.keys.length === 3 ? 0 : 1);
