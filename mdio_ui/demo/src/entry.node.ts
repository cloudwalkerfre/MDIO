// @ts-check
/**
 * entry.node —— 壳宿主（esmx 应用入口，行为面）。
 *
 * 协议事实在 package.json 的 esmx 段（协议唯一落点）；本文件只放行为：
 *   devApp  = 构建器（@esmx/vite + React；dev 与 build 都由它提供）
 *   server  = 唯一自写宿主件：网关同源代理 + COOP/COEP 响应头 + esmx.middleware（静态/编译）
 *             + esmx.render（SSR 兜底）
 *   postBuild = 生产静态壳：dist/client/index.html（官方 CSR 写法）
 * 浏览器只连本壳一个 origin；后端地址只出现在这里（§1.1 / §1.3 / 附录 B1·B4）。
 */
import fs from "node:fs";
import http from "node:http";
import { createRequire } from "node:module";
import { spawn } from "node:child_process";
import net from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";
import type { EsmxOptions } from "@esmx/core";
import { discoverWtModules } from "./std/host/wtModules";

/** 仓库根（src → demo → mdio_ui → 仓库根）：工作树模块发现用。 */
const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

/** 用于把 shim 里的裸名解析到**真包的 ESM 入口**（见 devApp 里的 `mdio-shim-inline` 插件）。 */
const nodeRequire = createRequire(import.meta.url);

const HOST = "127.0.0.1";
// ── 参数化启动：args > env > default ──
// esmx CLI 的 dev 分支不消费 argv（@esmx/core cli.mjs 只把 argv 留给 validate），本文件与 CLI
// 同进程 ⇒ 直读 process.argv。用法：esmx dev --mdio-port 5179 --mdio-backend-port 8021。
// 不带 args 且无 env 时 = 历史默认值，行为逐字节一致。
const argFlag = (name: string) => {
  const i = process.argv.indexOf("--" + name);
  return i >= 0 && i + 1 < process.argv.length ? process.argv[i + 1] : null;
};
const PORT = Number(argFlag("mdio-port") || process.env.MDIO_DEMO_PORT || 5178);
const BACKEND_HOST = "127.0.0.1";
const BACKEND_PORT = Number(argFlag("mdio-backend-port") || process.env.MDIO_DEMO_BACKEND_PORT || 8011);
const BACKEND = "http://" + BACKEND_HOST + ":" + BACKEND_PORT;

/** 同源代理表（语义 = 旧 vite.config.js server.proxy 逐字一致：前缀命中即转发）。 */
const PROXY_PATHS = ["/exec", "/ui-assets", "/fs", "/control", "/gateway", "/telemetry", "/mcp", "/ws"];
/** 走协议升级（WebSocket）的代理前缀；其余升级请求一律断开。 */
const UPGRADE_PATHS = ["/ws"];

/** COOP/COEP 响应头（SharedArrayBuffer 的前提；语义 = 旧 vite.config.js server.headers 逐字一致）。 */
const CROSS_ORIGIN_ISOLATION: Record<string, string> = {
  "Cross-Origin-Opener-Policy": "same-origin",
  "Cross-Origin-Embedder-Policy": "require-corp",
};

/**
 * 根 URL 空间的静态目录（兜底）不存在（wasm 收编进 ui_wasm 适配器）：
 * wasm / crdt worker 改由 esmx 模块资产服务（/ui_wasm/…），不再有根空间裸路径。
 */
const CONTENT_TYPES: Record<string, string> = {
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".map": "application/json; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".html": "text/html; charset=utf-8",
};

/**
 * dev 期 React Fast Refresh 前置码（插件公开串 viteReact.preambleCode，空串 = 生产或兜底路径）。
 *
 * 为什么自注入：rc.html 由 entry.server 手写 ⇒ 不过 Vite 的 HTML 管线，
 * @vitejs/plugin-react 的 transformIndexHtml 自动注入不会发生；缺它则 JSX 模块 evaluate 时
 * 抛「can't detect preamble」（vite:react-refresh 包装的前置检查）。
 */
let devPreamble = "";

/** dev：把 Vite 改写出来的 `/@fs/<官方件源码>` 与 `/@id/<裸名>` 改回裸名（浏览器用 importmap 解析）。
 *  在 devApp 里按官方件的 package.json 填，由 server 的中间件在**响应体**上应用（Vite 的 import 改写
 *  发生在所有 post 插件之后，插件层拿不到 —— 实测：post 插件的 transform 看到的仍是裸名）。 */
let bareRules: Array<{ re: RegExp; bare: string }> = [];

function withDevPreamble(html: string): string {
  const tag = '<script type="module">' + devPreamble + "</script>";
  return html.includes("</head>") ? html.replace("</head>", tag + "</head>") : tag + html;
}

/**
 * 共享单例的接线（按 esmx 官方模型；没有 shim 机制与 importmap 覆盖）。
 *
 * 官方模型（llms.md「接线是推导出来的」）：每个共享包只有一个**所有者**（平台的 `provides`），
 * 消费方只写 `uses`；**裸 specifier 在供给表里 ⇒ 外部化，接到所有者**。生产构建一直如此
 *（实测 `dist/client/src/entry.client.*.mjs`：产物里是**逻辑名** `ui_core/boot`·`ui_core/uiMd`…，
 *  **没有**路径形态的 `ui_core/services/*.js` ⇒ 那 13 把路径形态别名键已从 `ui_core/package.json` 删除）。
 *
 * 不在运行期**覆盖 importmap**：覆盖会把裸名指到宿主 shim（`src/std/shims/*.ts`）——
 * 起因是「平台产物内联了自己那份 React、与宿主 renderer 不同源 ⇒ useRef 为 null」。
 * 那是**混合接线**（一半走宿主、一半走平台）造成的两份；这里不覆盖 importmap、
 * 让平台做唯一所有者，并在 **dev 也把 `uses`/共享包外部化**（见 devApp 里的 externalizeUses）
 * ⇒ dev 与生产同形：Vite 只处理宿主自己的源码，其余全部由 esmx 的 importmap 解析。
 */


/**
 * 生产壳的 CSS 联邦：官方 mod 由**静态图**装载（HTML 里有它们的 modulepreload），
 * 但各 mod 是**独立构建** ⇒ 它们的 CSS 进不了外壳的 entry.css（实测 dist/client 只有 entry.*.css），
 * 而 dev 期这些样式由 Vite 用 JS 注入 ⇒ 生产页**完全没样式**（实测截图：文件树退化成平铺列表）。
 * 修法：读各 mod 产物 manifest 的 `chunks[*].css`，往生产 HTML 补 `<link rel="stylesheet">`。
 * 运行时装载的树内 mod 不走这条：它们带 `sheet`，由 modLoader → adoptCssMod 采纳（见 cssAdopt.js 的设计注释）。
 */
function modCssLinks(root: string): string[] {
  const out: string[] = [];
  const modsDir = path.resolve(root, "../lib/mods");
  let names: string[] = [];
  try { names = fs.readdirSync(modsDir); } catch { return out; }
  for (const m of names) {
    try {
      const j = JSON.parse(fs.readFileSync(path.join(modsDir, m, "dist/client/manifest.json"), "utf8"));
      for (const c of Object.values(j.chunks || {}) as Array<{ css?: string[] }>) {
        for (const css of (c.css || [])) out.push("/" + m + "/" + css);
      }
      // 兜底（实测）：**chunks 表不一定记录到每一张 CSS** —— ui_core 有 17 个 chunks，
      // 但没有一个带 `ErrorBoundary.css`（文件产出了、manifest 没登记）⇒ 页面里没有这条 <link> ⇒
      // `.dv-mini`（Log 面板的 清空/底部）与 `.dv-eb`（错误卡）整片掉回浏览器默认样式
      //（业主报「[清空 底部] 的样式怎么没了」）。⇒ **无条件**扫一遍 mod 的 dist/client 把 .css 全带上；
      // 重复靠下面的 Set 去重，幂等。
      for (const f of fs.readdirSync(path.join(modsDir, m, "dist/client"))) {
        if (f.endsWith(".css")) out.push("/" + m + "/" + f);
      }
    } catch { /* 无产物/manifest 的 mod 跳过 */ }
  }
  return [...new Set(out)];
}
/** 把各 mod 的 CSS 以 <link> 挂进生产 HTML（幂等：已在 HTML 里的不重复加）。 */
function withModCss(html: string, root: string): string {
  const links = modCssLinks(root);
  if (!links.length) return html;
  const tags = links.filter((u) => html.indexOf(u) < 0).map((u) => '<link rel="stylesheet" href="' + u + '">').join("");
  if (!tags) return html;
  return html.includes("</head>") ? html.replace("</head>", tags + "</head>") : tags + html;
}

function isProxied(url: string): boolean {
  return PROXY_PATHS.some((prefix) => url.startsWith(prefix));
}

/** 从 URL query 取 wt（'?wt=<uid>'；缺省空 ⇒ genesis）。页面与同源取件共用同一套坐标。 */
function wtFromUrl(url: string | undefined): string {
  try {
    const w = new URL(url || "/", "http://127.0.0.1").searchParams.get("wt");
    return w && w.trim() ? w.trim() : "";
  } catch { return ""; }
}

/**
 * 同源取件的 wt 来源 = **Referer 的 ?wt=**（每请求、无状态；不引 cookie、不引全局）。
 * 浏览器同源 fetch 默认带完整 Referer（strict-origin-when-cross-origin：同源即全 URL），
 * 所以子 wt 页面里的 /fs/tree 会自动落到同一个 wt；取不到就不加参数（后端默认 genesis）。
 */
function wtFromRequest(req: http.IncomingMessage): string {
  const ref = String(req.headers.referer || "");
  return ref ? wtFromUrl(ref) : "";
}

/**
 * 工作树模块的**挂载表**：只取已构建的（`built` = dist/client/manifest.json 存在）。
 * G1：未构建的树**不再静默消失**——每棵打一条明确告警（含构建命令），而不是让它在启动里无声无息。
 */
/**
 * **宿主侧按需编译**（主线"分布式的动态编译 + load"的宿主半）：发现到"只有源码 / 产物不可信"的包时，
 * **后台串行**触发一次构建；下一次请求的 import map 就会带上它（map 每请求生成 —— 实测）。
 * 语义边界：**非阻塞**（当前请求不等它，本次页面可能少一个 mod，日志写明）/ **串行**（同时只跑一个构建）/
 * **可关**（`MDIO_AUTO_BUILD=0`）。构建本身仍是 `scripts/build-wt-ui.mjs`（同一入口，戳与 hash 语义不变）。
 */
const autoBuildKicked = new Set<string>();
const autoBuildQueue: Array<{ name: string; root: string; official?: boolean }> = [];
let autoBuilding = false;
function autoBuildPump() {
  if (autoBuilding || autoBuildQueue.length === 0) return;
  const job = autoBuildQueue.shift() as { name: string; root: string; official?: boolean };
  autoBuilding = true;
  const t0 = Date.now();
  const demoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
  // **官方件与树内件用不同构建器**（B5 并入）：官方 mod 包（`mdio_ui/lib/mods/*`）走 `vp-build.mjs`
  //（`vp-build.mjs:11` 明写：树内 mod 不走它）；树内包仍走 `build-wt-ui.mjs`（戳/hash 语义不变）。
  const marker = job.official ? "[vp-build]" : "[build-wt-ui]";
  const args = [path.join(demoRoot, job.official ? "scripts/vp-build.mjs" : "scripts/build-wt-ui.mjs")].concat(job.official ? [] : [job.root]);
  console.log("[autoBuild] 开始编译 " + job.name + " ⇒ " + job.root + (job.official ? "（官方件 · vp-build）" : ""));
  const child = spawn(process.execPath, args, { cwd: demoRoot, stdio: ["ignore", "pipe", "pipe"] });
  let out = "";
  child.stdout.on("data", (b) => { out += String(b); });
  child.stderr.on("data", (b) => { out += String(b); });
  child.on("close", (code) => {
    autoBuilding = false;
    const line = out.split(String.fromCharCode(10)).filter((l) => l.indexOf(marker) >= 0).pop() || "";
    console.log("[autoBuild] " + (code === 0 ? "✓ " : "✗ ") + job.name + " · " + (Date.now() - t0) + "ms（下一次请求的 map 会带上它）" + (line ? " · " + line.trim().replace(marker + " ", "") : ""));
    if (code === 0) buildSeq += 1;   // 浏览器轮询它决定是否整页重载（见 watchModSources / withDevReload）
    autoBuildPump();
  });
}
function autoBuildKick(name: string, root: string, official = false) {
  if (String(process.env.MDIO_AUTO_BUILD || "1") === "0") return;
  if (autoBuildKicked.has(name)) return;
  autoBuildKicked.add(name);
  autoBuildQueue.push({ name, root, official });
  setImmediate(autoBuildPump);
}

/**
 * **dev 期"变更通知 + 整页重载"**（落地 · SURVEY 定稿的"借机制"，E8 实测闭环 123ms/小 mod、2.4s/真 mod）。
 *
 * 为什么需要：dev 期改树内 mod 的**源码**，产物不会自己变 —— 旧行为是"手动重编 + 手动刷新"。
 * 机制（不依赖 Vite 内部）：① `fs.watch` 每个树内包的 `src/`（**真实路径**，不经 ui/node_modules 的软链 ⇒
 * 不需要 vite-plugin-symlink-watcher 那一类补丁）② 去抖 150ms ③ 走**同一条**构建队列（`build-wt-ui.mjs`，戳/hash 语义不变）
 * ④ 构建成功 ⇒ `buildSeq += 1` ⑤ 页面注入的脚本轮询 `/__mdio/dev-seq`，序号一变就 `location.reload()`
 *   （import map 不可改是规范级事实 ⇒ 只能整页重载，这正是 SURVEY 第三/七轮的结论）。
 *
 * 可关：`MDIO_WATCH_MODS=0`。只监视**树内** mod（官方件走宿主自己的 Vite 管线，本来就有 HMR/Fast Refresh）。
 */
let buildSeq = 0;
// 默认开（dev 手跑要的就是"改源码 ⇒ 自动重编 ⇒ 整页重载"）；**自动化运行**（verify-all/verify-specials 设的
// `MDIO_TESTING=1`）默认关 —— 那些 rehearsal 会写 mod 源码，重载会把断言打断（实测抖动）。
// 显式 `MDIO_WATCH_MODS=1/0` 永远优先。
const WATCH_MODS = String(process.env.MDIO_WATCH_MODS || (process.env.MDIO_TESTING ? "0" : "1")) !== "0";
/** @type {import("node:fs").FSWatcher[]} */
const modWatchers: fs.FSWatcher[] = [];
/** @type {Map<string, NodeJS.Timeout>} */
const modDebounce = new Map();
/** watch 触发的重建必须绕过"只踢一次"的守卫。 */
function kickRebuild(name: string, root: string, official = false) {
  autoBuildKicked.delete(name);
  autoBuildQueue.push({ name, root, official });
  setImmediate(autoBuildPump);
}
function watchModSources() {
  if (!WATCH_MODS) { console.log("[watch] 关闭（MDIO_WATCH_MODS=0）⇒ 改源码后请手动 node scripts/build-wt-ui.mjs <pkg>"); return; }
  for (const [name, info] of Object.entries(discoverWtModules(REPO_ROOT))) {
    const pkgDir = info.pkgDir || info.root;
    const src = path.join(pkgDir, "src");
    try {
      if (!fs.existsSync(src)) continue;
      modWatchers.push(fs.watch(src, { recursive: true }, () => {
        const t = modDebounce.get(name);
        if (t) clearTimeout(t);
        modDebounce.set(name, setTimeout(() => {
          modDebounce.delete(name);
          console.log("[watch] 源码变了 ⇒ 重编 " + name + "（" + pkgDir + "）");
          kickRebuild(name, pkgDir);
        }, 150));
      }));
    } catch (e) { console.warn("[watch] 无法监视 " + name + " · " + String((e as Error).message)); }
  }
  // **官方件（`mdio_ui/lib/mods/*`）不盯 —— 它们在 dev 下由宿主自己的 Vite 管线管**（校正）：
  //   实测：改 `ui_core/src/services/uiMd.js` ⇒ 宿主日志 `[vite] (client) page reload /…/lib/mods/ui_core/…`；
  //   改 `ui_std/src/comps.js` ⇒ `[vite] (client) hmr update`；**全程零构建、页面照旧**。
  //   ⇒ **dev 下不要给官方件跑构建**：`vp-build` 是**生产**构建，在 dev 跑会把生产代际与 dev 混在一起 ——
  //   那正是「页面白屏（`TypeError: r is not a function`，`#root` 空白）」的成因（当天据此打白过两次）。
  //   ⇒ **生产**（Node / `esmx start`，吃的是各包 `dist`）改官方件才需要：五包 `vp-build` + 重启。
  //   树内件不受影响：仍由本处 `build-wt-ui` 自动重建 + `dev-seq` 触达整页重载。
}

function wtModuleLinks(repoRoot: string): Record<string, string> {
  const links: Record<string, string> = {};
  for (const [name, info] of Object.entries(discoverWtModules(repoRoot))) {
    const usable = info.usable === null ? info.built : info.usable;
    if (usable) links[name] = info.dist;
    else if (info.usable === false) { autoBuildKick(name, info.pkgDir || info.root); console.warn("[wtModules] " + name + " 的产物不可信（" + info.reason + "）⇒ 本次不挂载；已请宿主后台编译（见 [autoBuild]）；手工重编：node mdio_ui/demo/scripts/build-wt-ui.mjs " + info.root); }
    else { autoBuildKick(name, info.pkgDir || info.root); console.warn("[wtModules] " + name + " 只有源码、未构建 ⇒ 本次不挂载；已请宿主后台编译（见 [autoBuild]）；手工构建：node mdio_ui/demo/scripts/build-wt-ui.mjs " + info.root); }
  }
  return links;
}



/** 只读链：后端 /control/wt/chain?uid=…（与 ui_dirs_for_wt 同源）；取不到返回 null（客户端退化 [wt]）。 */
function fetchChain(uid: string): Promise<unknown> {
  return new Promise((resolve) => {
    const up = http.request(
      { hostname: BACKEND_HOST, port: BACKEND_PORT, path: "/control/wt/chain?uid=" + encodeURIComponent(uid), method: "GET" },
      (r) => {
        let body = "";
        r.setEncoding("utf8");
        r.on("data", (c) => { body += c; });
        r.on("end", () => { try { resolve(JSON.parse(body)); } catch { resolve(null); } });
      },
    );
    up.on("error", () => resolve(null));
    up.end();
  });
}

/** 链数据内联进 HTML（data 元素，不是全局探针）：客户端读 #mdio-wt-chain，不再发请求。 */
/**
 * 把**宿主实际挂载的那棵树**注入页面：`MDIO_VFS_MOUNTS` 是宿主/网关启动时定的，
 * 网关一次只挂一棵（`up-all.mjs:86`），而切换器列的是**注册表**里的所有树 ⇒ 切到"在册未挂载"的树
 * 会得到一个能打开、后端却不服务的 URL（业主实测：切 exp_pages ⇒ ?wt=c8Page01 ⇒ 卡在装载态）。
 * 客户端据此在**连 WS 之前**判死并给出可操作提示（见 entry.client.ts 的 wtGate 步）。
 */
function injectHostMount(html: string, mount: string): string {
  const json = JSON.stringify(String(mount || "")).replace(/</g, "\\u003c");
  const tag = '<script type="application/json" id="mdio-host-mount">' + json + "</script>";
  return html.includes("</head>") ? html.replace("</head>", tag + "</head>") : tag + html;
}

function injectWtChain(html: string, chain: unknown): string {
  const json = JSON.stringify(chain == null ? {} : chain).replace(/</g, "\\u003c");
  const tag = '<script type="application/json" id="mdio-wt-chain">' + json + "</script>";
  return html.includes("</head>") ? html.replace("</head>", tag + "</head>") : tag + html;
}

/** 同源转发（保留原 method/query/请求头与响应状态/响应头）。 */
function proxyHttp(req: http.IncomingMessage, res: http.ServerResponse): void {
  const target = new URL(req.url || "/", BACKEND);
  // 子 wt 页面的同源取件带不动 wt ⇒ 用 Referer 补：/fs/tree 未显式给 wt_uid 时按页面 wt 落。
  if (target.pathname === "/fs/tree" && !target.searchParams.has("wt_uid")) {
    const wt = wtFromRequest(req);
    if (wt) target.searchParams.set("wt_uid", wt);
  }
  const upstream = http.request(
    {
      hostname: target.hostname,
      port: target.port,
      path: target.pathname + target.search,
      method: req.method,
      headers: req.headers,
    },
    (up) => {
      res.writeHead(up.statusCode || 502, up.headers);
      up.pipe(res);
    },
  );
  upstream.on("error", (e) => {
    if (!res.headersSent) res.writeHead(502, { "content-type": "text/plain; charset=utf-8" });
    res.end("proxy error: " + String((e && e.message) || e));
  });
  req.pipe(upstream);
}

/** 升级转发：原始握手字节直通后端，成败由后端应答决定（WS 归 wasm，壳只搬字节）。 */
function proxyUpgrade(req: http.IncomingMessage, socket: net.Socket, head: Buffer): void {
  const upstream = net.connect(BACKEND_PORT, BACKEND_HOST, () => {
    const lines = ["GET " + (req.url || "/") + " HTTP/1.1"];
    for (let i = 0; i + 1 < req.rawHeaders.length; i += 2) {
      lines.push(req.rawHeaders[i] + ": " + req.rawHeaders[i + 1]);
    }
    upstream.write(lines.join("\r\n") + "\r\n\r\n");
    if (head && head.length > 0) upstream.write(head);
    upstream.pipe(socket);
    socket.pipe(upstream);
  });
  upstream.on("error", () => socket.destroy());
  socket.on("error", () => upstream.destroy());
  // 上游优雅关闭（FIN）时，pipe 只做 socket.end() 半关 —— Chrome 不 fire WS close（实测 Network.webSocketClosed=0），
  // wasm 读循环永远等不到 EOF ⇒ 引擎不重连。用 destroy（RST）打醒浏览器：close(1006) ⇒ EOF ⇒ 引擎走重连自愈。
  upstream.on("close", () => socket.destroy());
}

/**
 * vendor 大件静态路由（/vendor/*）：monaco 这类自包含大 bundle 由壳直出，不进 vite 的模块链路
 * （public 不能从源码 import；预打包/现场转换两条路实测都会挂死主线程）。前端用运行时取件 + Blob import。
 */
function serveVendorFile(req: http.IncomingMessage, res: http.ServerResponse, root: string): boolean {
  if (req.method !== "GET" && req.method !== "HEAD") return false;
  const url = (req.url || "").split("?")[0];
  if (!url.startsWith("/vendor/")) return false;
  const vendorRoot = path.resolve(root, "vendor");
  const file = path.resolve(vendorRoot, decodeURIComponent(url.slice("/vendor/".length)));
  if (file !== vendorRoot && !file.startsWith(vendorRoot + path.sep)) {
    res.statusCode = 403;
    res.end();
    return true;
  }
  fs.stat(file, (err, stats) => {
    if (err || !stats.isFile()) {
      res.statusCode = 404;
      res.end();
      return;
    }
    res.setHeader("content-type", CONTENT_TYPES[path.extname(file)] || "application/octet-stream");
    res.setHeader("cache-control", "no-cache");
    if (req.method === "HEAD") {
      res.end();
      return;
    }
    fs.createReadStream(file).pipe(res);
  });
  return true;
}

/** 根空间静态：命中返回 true（已接管响应），未命中返回 false（交给 esmx.middleware）。 */
function servePublicFile(req: http.IncomingMessage, res: http.ServerResponse, root: string): boolean {
  if (req.method !== "GET" && req.method !== "HEAD") return false;
  const url = (req.url || "").split("?")[0];
  if (url.startsWith("/wasm/") || url.startsWith("/crdt/")) { res.statusCode = 404; res.end(); return true; }   // 交给 esmx 资产面（防回潮 = 404 大声）
  return false;   // 其余一律交给 esmx.middleware（原 public 静态目录已随 wasm 收编清空）
}

/**
 * dev HTML 注入：轮询 `/__mdio/dev-seq`，序号一变就整页重载（见 watchModSources 的说明）。
 * 为什么是轮询而不是 WS：宿主已经有网关 WS（`/ws`）走代理，另起一条协议只为"重载"不值当；
 * 700ms 轮询在本地是零成本，且**首次只记录序号、不重载**（避免页面刚开就自我刷新）。
 */
function withDevReload(html: string): string {
  if (!WATCH_MODS) return html;
  const s = "<script>(function(){var seen=null;setInterval(function(){fetch('/__mdio/dev-seq',{cache:'no-store'})" +
    ".then(function(r){return r.json()}).then(function(j){if(seen===null){seen=j.seq;return;}if(j.seq!==seen){seen=j.seq;" +
    "console.log('[mdio] 树内 mod 已重建 ⇒ 整页重载 seq='+j.seq);location.reload();}}).catch(function(){});},700);})();</script>";
  return html.indexOf("</body>") >= 0 ? html.replace("</body>", s + "</body>") : html + s;
}

export default {
  root: undefined,
  /**
   * wt 链上的 ui 模块（§2.2 第②步）：links = 模块名 → **产物目录**（含 client/ 与 server/ 的那层）。
   * 路径由本文件位置推出（不依赖 cwd：e2e 的 ⑯ 在影子工程里构建宿主，cwd 不同）。
   * 挂载表由 package.json 的 esmx.uses 驱动，links 只负责给出 artifactDir：
   * 两边都要有——缺 uses 则本 links 被静默忽略（@esmx/core declaration/resolver.mjs:241-249）。
   */
  modules: {
    links: {
      // 平台（单例供给）：固定入口，不是工作树模块。
      mdio_ui: path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../lib/mods/ui_base/dist"),
      // 工作树 UI 模块：**由文件系统发现**（§25.3 原则一：挂载表不写在应用里），
      // 与 package.json 的 esmx.uses 共用同一份发现（scripts/sync-wt-uses.mjs）。
      // 客户端再按 /control/wt/chain 的链序挑（子覆父）。「每 wt 一个 Esmx 实例」是后续形态。
      ...wtModuleLinks(REPO_ROOT),
    },
  },
  async devApp(esmx) {
    // React 应用用 createViteApp（createViteHtmlApp 是无框架版，不适用）
    const { createViteApp } = await import("@esmx/vite");
    const react = (await import("@vitejs/plugin-react")).default;
    const code = (react as { preambleCode?: unknown }).preambleCode;
    const hasPreamble = typeof code === "string" && code.includes("__BASE__");
    devPreamble = hasPreamble ? String(code).replace("__BASE__", esmx.basePath) : "";
    // tailwind 本体已随**平台样式层**走：ui_base/style.css 在 ui_base 的构建里编译成 esmx CSS chunk
    // （构建挂在 build-wt-ui.mjs），host 经 rc.css() 注入 <link> —— 这里不再挂插件。
    return createViteApp(esmx, {
      config(ctx) {
        // Vite 缓存目录显式落到**本工程内**：默认是 node_modules/.vite，而 e2e 影子工程的 node_modules
        // 是指向真工程的软链 ⇒ dev 与影子 build 共用同一份缓存，构建会把 dev 的连接打坏（实测 ECONNRESET，
        // 表现为 e2e ⑰/⑱ 在 ⑯ 之后 fetch 失败）。
        ctx.config.cacheDir = path.resolve(esmx.root, ".vite-cache");
        // （此处的 `resolve.alias` 共享名锚点**实测无效** —— esmx 随后覆盖 resolve，Vite 侧不生效。）
        // 真正生效的修法在**文件系统层**：给 `ui_core`/`ui_devview` 这类「自己没有 node_modules」的包在其包内建
        // 共享包链接（react/react-dom/mobx/mobx-react-lite/mobx-state-tree → 宿主 demo 的那一份）；
        // 已固化进 `scripts/ensure-tree-deps.mjs` 的「共享单例落点」一节。        // .tmp 下全是测试产物（截图、探针脚本、临时工程）⇒ 挡在 Vite 自己的 watcher 之外（卫生措施）。
        // 注：**不是**"dev 渲染被打断"那条的修法 —— 那条来自 **esmx 自己的** watcher（构建产物写进 dev 根会触发
        // 一次全量 page reload），Vite 的 server.watch.ignored 管不到它；修法是让构建产物落在 dev 根之外（见 test/e2e.mjs 影子工程）。
        // **服务允许列表（实测事故的修法）**：Vite 的 `server.fs.allow` 默认按"探测到的 workspace 根"
        // 自动推断；仓库根那份**误装的** package.json/node_modules 被删掉之后，探测结果缩到 `mdio_ui/demo`，
        // 于是 `lib/mods/**` 与 `worktrees/**` 里的源文件（被 `link:` 进宿主图的包）**不再被允许服务**：
        // 请求 `/demo/@fs/…/ui_devview/src/DevView.jsx` 返回 **200 text/html**（落到宿主兜底页）⇒ 浏览器报
        // `Failed to load module script … MIME type of "text/html"` ⇒ 客户端模块压根不执行（页面停在装载态）。
        // 显式允许**仓库根**（它覆盖 lib/mods 与 worktrees，也覆盖将来的树）。
        ctx.config.server = Object.assign({}, ctx.config.server, {
          fs: Object.assign({}, (ctx.config.server && ctx.config.server.fs) || {}, { allow: [path.resolve(esmx.root, "../..") ] }),
          watch: Object.assign({}, (ctx.config.server && ctx.config.server.watch) || {}, {
            ignored: [
              ...((ctx.config.server && ctx.config.server.watch && ctx.config.server.watch.ignored) || []),
              "**/.tmp/**",
              "**/.vite-cache/**",
            ],
          }),
        });
        // monaco **必须 exclude 出预打包**：实测 esbuild 预打包出的 dep chunk 会让 editor.create 挂死主线程，
        // 而逐模块原始 ESM（vite 现场转换）正常（0.52.2 裸测 CREATE OK）。include 方案会反过来阻塞 dev 服务器。
        ctx.config.optimizeDeps = Object.assign({}, ctx.config.optimizeDeps, {
          exclude: [
            ...((ctx.config.optimizeDeps && ctx.config.optimizeDeps.exclude) || []),
            "monaco-editor",
            "@monaco-editor/react",
            // **不要把共享单例（react/react-dom/mobx 系）加进 exclude**（实测）：
            // shim（`src/std/shims/*.ts`）的实现前提是"裸名解析到 Vite **预打包的那一份**"
            //（见 `shims/react.ts` 注释：renderer 就是它、hooks 的 dispatcher 由 renderer 写入）。
            // 把 react 排除出预打包后，shim 自己的 `import "react"` 会指回 shim 自身 ⇒ 自引用 ⇒ 命名空间/default 为 null
            // ⇒ `mobx-react-lite` 里 `React.useRef` 报 `Cannot read properties of null` ⇒ 文档面被 ErrorBoundary 隔离
            // ⇒ `no .dv-doc-inner`（dev 专属；prod 由提供者给单实例，故 verify-prod 一直绿）。
          ],
        });
        // （原先这里有一段"共享 shim 的生产自引用"补丁 —— 重写 `build.rollupOptions.external`，
        //  只在被 shim 导入时把 react/mobx 裸名强打进产物。shim 机制已按官方模型删除（见上方说明）⇒
        //  自引用前提消失，整段随之删除；共享包由平台 `provides` 唯一供给、esmx 原样外部化。）
        // 【官方模型】dev 也只让 Vite 处理**宿主自己的源码**：
        // `uses` 的远程 + 被 `provides` 的共享包一律 external ⇒ 浏览器按 esmx 生成的 importmap 解析
        //（官方 llms.md「查找规则」：bare specifier 在供给表里 ⇒ 外部化，接到所有者）。
        // 生产构建本来就是这样（实测产物里是逻辑名 `ui_core/boot` 一类，见上方说明）；
        // dev 若让 Vite 解析这些裸名 ⇒ 官方件源码被拉进 Vite 图 ⇒ **同一模块两份实例**
        //（实测：mobx/react 各两份、DocPane/NodeView 各两份；这也是"文档面全量重渲染"无法归因的根）。
        // 注：本 config 钩子在 devApp 里 ⇒ **只在 dev 跑**（`esmx build` 不走这里）。
        // 不要再拿 `ctx.config.command` 判 dev：实测该字段是 **undefined**，条件会永远为假。
        if (ctx.buildTarget === "client") {
          // "源文件绝对路径 → 裸名"映射：读各模块 package.json 的 esmx.exports（逻辑键 = 公开名）。
          const pairs: Array<[string, string]> = [];
          const addPkg = (pkgDir: string) => {
            try {
              const pj = JSON.parse(fs.readFileSync(path.join(pkgDir, "package.json"), "utf8"));
              const name = String((pj && pj.name) || "");
              const ex = (pj && pj.esmx && pj.esmx.exports) || {};
              for (const [key, val] of Object.entries(ex)) {
                const rel = typeof val === "string" ? val : ((val as any) && (val as any).client) || null;
                if (!rel || !name) continue;
                pairs.push([path.resolve(pkgDir, String(rel)), name + key.slice(1)]);
              }
            } catch (e) { /* 无声明/无产物 ⇒ 跳过 */ }
          };
          try {
            const modsRoot = path.resolve(esmx.root, "../lib/mods");
            for (const d of fs.readdirSync(modsRoot)) addPkg(path.resolve(modsRoot, d));
          } catch (e) { /* ignore */ }
          pairs.sort((x, y) => y[0].length - x[0].length);   // 长路径优先，避免前缀互截
          const esc = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
          const rules = pairs.map(([abs, bare]) => ({
            // 两种形态都改回裸名：① Vite 解析成源码的 `/@fs/<绝对路径>`；② external 桩的 `/@id/<裸名>`
            re: new RegExp("(?:/demo)?/(?:@fs" + esc(abs) + "|@id/" + esc(bare) + ")(?:\\?[^\"'\\s)]*)?", "g"),
            bare: bare,
          }));
          // ② **被 provides 的共享包**：Vite 会把它们预打包成 `/demo/node_modules/.vite/deps/<名>.js`
          //    ⇒ 也必须改回裸名，否则 renderer 用 Vite 那份、组件用平台那份 ⇒ **两份 React**
          //    （实测报 `Cannot read properties of null (reading 'useState')`：dispatcher 未设）。
          let providedPkgs: string[] = [
            "react", "react/jsx-runtime", "react/jsx-dev-runtime",
            "react-dom", "react-dom/client",
            "mobx", "mobx-react-lite", "mobx-state-tree",
          ];
          try {
            const basePj = JSON.parse(fs.readFileSync(path.resolve(esmx.root, "../lib/mods/ui_base/package.json"), "utf8"));
            const p = basePj && basePj.esmx && basePj.esmx.provides;
            if (Array.isArray(p) && p.length) providedPkgs = p.map(String);
          } catch (e) { /* 用内建默认表 */ }
          const depRules = providedPkgs.map((name) => ({
            re: new RegExp("(?:/demo)?/node_modules/\\.vite/deps/" + esc(name.replace(/\//g, "_")) + "\\.js(?:\\?[^\"'\\s)]*)?", "g"),
            bare: name,
          }));
          bareRules = rules.concat(depRules);   // ← 两组都要：① 把官方件源码 URL 改回裸名；② 把共享包的预打包 URL 改回裸名
          // 但**modules 自身的源码响应不得改写**：模块内部的相对导入（例：ErrorBoundary.jsx →
          // `../services/monitor.js`）改用裸名后会变成 `ui_core/services/monitor.js`，而**别名键进不了
          // importmap**（映射按文件去重）⇒ 浏览器报 `Failed to resolve module specifier`（实测事故）。
          // ⇒ 判据见下方中间件：URL 命中 `/@fs/**/lib/mods/**` 的一律原样放行。
          const bareSet = new Set(pairs.map((p) => p[1]));
          void bareSet;
          ctx.config.plugins = ctx.config.plugins || [];
          // 实测：**不要**用 `resolveId → { external: true }` —— Vite dev 会为它生成
          // `/demo/@id/<裸名>` 的"浏览器兼容桩"，运行时一访问就抛；boot 把它吞在 promise 里 ⇒ 页面
          // **静默卡在装载**（零报错、零失败请求，最难查）。正解 = 保持裸名不动，让浏览器用 importmap 解析。
          ctx.config.plugins.push({
            name: "mdio-bare-mod-specifiers",
            enforce: "post",
            transform(code: string, id: string) {
              let out = code;
              let hits = 0;
              for (const r of rules) { r.re.lastIndex = 0; if (r.re.test(out)) { r.re.lastIndex = 0; out = out.replace(r.re, r.bare); hits += 1; } }
              if (process.env.MDIO_TX_DEBUG === "1" && (hits || id.indexOf("entry.client") >= 0)) {
                console.log("[tx] " + id.slice(-46) + " · hits=" + hits + " · 含/@fs/=" + (code.indexOf("/@fs/") >= 0));
              }
              return out === code ? null : out;
            },
          });
        }

        // （`mdio-md-jsx` 宿主端点不存在：md 代码块回归装载线自己的那一条路 —— `exec 取原文 → Blob → import`，
        // JSX 需要编译时由装载线内部转一次，不再另起通路。）
        // （本版 5.2.0 无 fastRefresh 选项可关：dist/index.js:148 的 skipFastRefresh 由 config 推导。）
        if (hasPreamble) {
          ctx.config.plugins.push(react());
          return;
        }
        console.warn("[shell] @vitejs/plugin-react.preambleCode 不可用 → 本次 dev 不挂 react 插件（无 Fast Refresh）");
      },
    });
  },
  async server(esmx) {
    /**
     * 缺件降级（业主实测暴露）：挂载表里某项的产物**在宿主运行期间**被删掉
     *（演练清场的窗口期、或手工删目录），esmx 取它的 manifest 会抛 ENOENT —— 旧行为把整页打成
     * `render error:` 500。这里在**每次渲染前**把"manifest 已不存在"的项从挂载表摘掉：
     * 页面照常渲染（该模块从地图里消失），日志留一条明确告警。
     * 为什么必须放运行期：这类删除启动时看不到；而 esmx 的 manifest 列表是按 env 缓存的。
     */
    const pruned = new Set();
    const pruneDeadLinks = () => {
      const links = (esmx as any)?.moduleConfig?.links as Record<string, { client?: string; server?: string }> | undefined;
      if (!links) return;
      for (const [name, item] of Object.entries(links)) {
        const dir = String((item && (item.client || item.server)) || "");
        if (!dir) continue;
        try {
          if (fs.existsSync(path.join(dir, "manifest.json"))) continue;
          delete (links as any)[name];
          if (!pruned.has(name)) { pruned.add(name); console.warn("[shell] 缺件：挂载表里的 " + name + " 产物不存在（" + dir + "）⇒ 本页摘掉它，其余照常。修法：node scripts/sync-wt-uses.mjs && node scripts/up-all.mjs"); }
        } catch (e) { /* 读不到的当健在，交给下面的兜底 */ }
      }
    };
    const server = http.createServer((req, res) => {
      // 【官方模型 · dev 只让 Vite 处理 mdio_ui 自己的源码】把模块响应里的官方件 URL 改回**裸名**，
      // 浏览器按 esmx 的 importmap 解析 ⇒ 官方件不进 Vite 图（实测：源码图模块 39 → 0）。
      // 为什么在这一层：Vite 的 import 改写发生在**所有 post 插件之后**（实测 post 插件的 transform
      // 看到的仍是裸名）；服务端响应体是最后一个能改的地方。只对 200 的 JS 响应生效；
      // 改写会变长度 ⇒ 必须去掉 Content-Length 让 Node 用 chunked 发送。
      const _reqUrl = req.url || "";
      const _isModSource = _reqUrl.indexOf("/@fs/") >= 0 && _reqUrl.indexOf("/lib/mods/") >= 0;
      if (bareRules.length && !_isModSource && /\.(ts|tsx|jsx|js|mjs)(\?|$)/.test(_reqUrl)) {
        const _write = res.write.bind(res);
        const _end = res.end.bind(res);
        let _buf = "";
        (res as any).write = (chunk: any) => { _buf += typeof chunk === "string" ? chunk : Buffer.from(chunk).toString("utf8"); return true; };
        (res as any).end = (chunk?: any) => {
          if (chunk) _buf += typeof chunk === "string" ? chunk : Buffer.from(chunk).toString("utf8");
          // **非 200 一律原样放行**（实测事故）：Vite 对重复请求回 **304**（服务端缓存命中，无 body）。
          // 旧写法对 304 也走"缓冲 ⇒ 写回"，等于给 304 塞了空 body ⇒ 浏览器拿到坏模块 ⇒ 页面**静默卡在装载**
          //（症状：首次加载正常，预热/二次请求之后整片红；`verify-all` 的预热正好触发）。
          if (res.statusCode !== 200) {
            try { res.removeHeader("content-length"); } catch (e) { /* ignore */ }
            if (chunk) _write(_buf, "utf8");
            return _end();
          }
          if (_buf) {
            for (const r of bareRules) { r.re.lastIndex = 0; if (r.re.test(_buf)) { r.re.lastIndex = 0; _buf = _buf.replace(r.re, r.bare); } }
          }
          if (process.env.MDIO_TX_DEBUG === "1") {
            const hitted = bareRules.filter((r) => { r.re.lastIndex = 0; return r.re.test(_buf); }).length;
            if (hitted || _reqUrl.indexOf("entry.client") >= 0 || _reqUrl.indexOf("@fs") >= 0) {
              console.log("[resp] " + _reqUrl.slice(0, 52) + " · len=" + _buf.length + " · status=" + res.statusCode + " · 命中规则=" + hitted + " · 含@fs=" + (_buf.indexOf("@fs") >= 0));
            }
          }
          try { res.removeHeader("content-length"); } catch (e) { /* ignore */ }
          _write(_buf, "utf8");
          return _end();
        };
      }
      for (const [name, value] of Object.entries(CROSS_ORIGIN_ISOLATION)) res.setHeader(name, value);
      const url = req.url || "/";
      // 宿主本地端点（**必须**放在 isProxied 之前：代理前缀里有 /control，别让它吃掉）
      if (url.startsWith("/__mdio/dev-seq")) {   // 重建序号（dev 期整页重载的判据源）
        res.setHeader("content-type", "application/json; charset=utf-8");
        res.setHeader("cache-control", "no-store");
        res.end(JSON.stringify({ seq: buildSeq, watching: modWatchers.length }));
        return;
      }
      if (isProxied(url)) {
        proxyHttp(req, res);
        return;
      }
      if (servePublicFile(req, res, esmx.root)) return;
      if (serveVendorFile(req, res, esmx.root)) return;
      esmx.middleware(req, res, async () => {
        try {
          pruneDeadLinks();
          const rc = await esmx.render({ params: { url: req.url } });
          const chain = await fetchChain(wtFromUrl(req.url) || "genesis");
          res.setHeader("content-type", "text/html; charset=utf-8");
          // 生产/预览壳不能再用 dev shim：`esmx preview` 也走这条渲染路径（实测它服务出去的 importmap
          // 把 mobx/react 指到 `/demo/src/std/shims/*.ts` ⇒ 预览页 404 卡在「正在装载…」）。
          // 判据 = NODE_ENV：dev 需要 shim（§58：dev 走产物实测页面完全不渲染），生产/预览需要 esmx 的原生映射。
          const isDevShell = process.env.NODE_ENV !== "production";
          const _html0 = injectHostMount(injectWtChain(rc.html, chain), process.env.MDIO_VFS_MOUNTS || "");   // 宿主（共享单例交给 esmx 的 importmap，见上方说明）
          // **dev 与 prod 一致**（官方「联邦 CSS」：远程自己打包样式、manifest 按 chunk 声明、壳注入 <link>）：
          // dev 靠 Vite 用 JS 注入样式；接线改走 esmx 产物后 Vite 不再经手这些件
          // ⇒ dev 必须和 prod 一样按 manifest 补 <link>，否则官方件无样式（实测：devview 判据 8 项红，
          //   footer 高 25/36、Monaco 无尺寸导致打字链路全部失败）。
          const html = withModCss(_html0, esmx.root);
          res.end(devPreamble ? withDevReload(withDevPreamble(html)) : html);   // dev 期额外接"重建 ⇒ 重载"轮询
        } catch (e) {
          res.statusCode = 500;
          res.setHeader("content-type", "text/plain; charset=utf-8");
          res.end("render error: " + String((e && e.message) || e));
        }
      });
    });
    server.on("upgrade", (req, socket, head) => {
      const url = req.url || "/";
      if (UPGRADE_PATHS.some((prefix) => url.startsWith(prefix))) {
        proxyUpgrade(req, socket as net.Socket, head);
        return;
      }
      socket.destroy();
    });
    watchModSources();   // dev 期：树内 mod 源码变更 ⇒ 自动重编 ⇒ 整页重载（MDIO_WATCH_MODS=0 可关）
    await new Promise<void>((resolve) => server.listen(PORT, HOST, resolve));
    console.log("[shell] mdio demo 壳宿主就绪 → http://" + HOST + ":" + PORT);
  },
  async postBuild(esmx) {
    // 官方 CSR 写法（附录 B4）：index.html 由 postBuild 生成，构建器不产
    const rc = await esmx.render();
    esmx.writeSync(esmx.resolvePath("dist/client", "index.html"), withModCss(rc.html, esmx.root));   // 生产壳：esmx 原生 importmap + mod CSS
  },
} satisfies EsmxOptions;
