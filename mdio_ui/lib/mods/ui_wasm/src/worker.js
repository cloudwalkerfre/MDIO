// wasm 的 worker 装配件（web 适配器 ui_wasm 版）：静态 import 胶水 → EditorCore 实例 →
// 建四块 SAB → connect → run_loop。消息协议：init/init_success/connect/connected/
// update_upstream_buffer/update_downstream_buffer/get_telemetry/telemetry_data/dispose。
// 与旧 demo/public/crdt/worker.js 的唯一差别：
//   ① import { init, EditorCore, editor_telemetry_data } from "./glue.js" 取代「动态 import(wasmUrl) + wasmModule.xxx」；
//   ② init 消息里的 wasmUrl 字段被忽略（保留在主线程协议里不破坏兼容）。
const DEBUG = false;

import { init, EditorCore, editor_telemetry_data } from "./glue.js";

let core = null;
let updater = null;

function post(msg) {
  self.postMessage(msg);
}

// 桥接：worker/wasm 的 console 转发到页面（页面级 CDP console 不含 worker ⇒ 排障与测量必需；永久保留）。
for (const lvl of ["log", "warn", "error"]) {
  const orig = console[lvl].bind(console);
  console[lvl] = (...args) => {
    try { orig(...args); } catch (_e) { /* 循环对象等照常落 */ }
    try {
      post({ type: "console", data: { level: lvl, text: "n=" + args.length + " " + args.map((a) => { if (a == null) return String(a); try { const s = typeof a === "string" ? a : JSON.stringify(a); return (s === undefined || s === "") ? "[" + typeof a + "]:" + String(a) : s; } catch (_e2) { return "[" + typeof a + "]:" + String(a); } }).join(" ") } });
    } catch (_e3) { /* 转发失败不影响 worker */ }
  };
}

// ── T1 打点（task-54）：worker 侧两点，切开 incr 的往返 ──────────────────────────────
// 语义：up  = 上游 SAB 出现主线程载荷（≈"准备发帧"）；down = 下游 SAB 出现网关回帧（≈"准备写回 SAB"）。
// 依据（协议）：ui_core/src/services/client.js 的 WRITE_PTR_IDX=0 / LPPB_RESERVED_SIZE=128（写后 Atomics.notify(0)）
//   与 lib/wasm/src/lppbuf.rs 同布局（wasm 写下游后 notify_with_count(0, u32::MAX)）⇒ 两侧都能零延迟唤醒。
// 规程（《可信打点规范》§3/§10）：Date.now() 与 [INC] wall= / wasmWallIn/Out 同钟；非阻塞 waitAsync，不挡 worker
//   消息循环；打点走 console 桥（既有导出通道），**不挂任何全局探针**（判官 R6a）。
const T1_SKIP = new Set([-1, 128, 2147483647]);   // 拆除(-1) / 复位(128=RESERVED) / 旧缓冲报废(i32::MAX)
const T1_WATCHED = new Set();
function t1Watch(sab, tag) {
  if (!sab || T1_WATCHED.has(sab)) return;
  let state;
  try { state = new Int32Array(sab); } catch (_e) { return; }
  T1_WATCHED.add(sab);
  let last = Atomics.load(state, 0);
  const arm = () => {
    let r;
    try { r = Atomics.waitAsync(state, 0, last, 2000); } catch (_e) { return; }
    Promise.resolve(r.async ? r.value : r).then(() => {
      const now = Atomics.load(state, 0);
      if (now !== last) {
        const prev = last;
        last = now;
        if (!T1_SKIP.has(now) && !T1_SKIP.has(prev)) console.log("[T1] " + tag + "=" + Date.now() + " w=" + now);
      }
      arm();
    }, () => {});
  };
  arm();
}

self.onmessage = async (e) => {
  const { type, data } = e.data;

  try {
    switch (type) {
      case "init": {
        const { peerId, upSab0, upSab1, downSab0, downSab1 } = data;

        await init();

        const onDownstreamUpgrade = (idx, newSab) => {
          post({ type: "update_downstream_buffer", data: { idx, sab: newSab } });
          t1Watch(newSab, "down");   // T1：下游换缓冲 ⇒ 新 SAB 也要挂
        };

        core = new EditorCore(peerId, upSab0, upSab1, downSab0, downSab1, onDownstreamUpgrade);
        updater = core.get_updater();
        // T1：四条 SAB 都挂上（双缓冲两条都要，切换时另一条会复位到 128，已在 T1_SKIP 里过滤）
        t1Watch(upSab0, "up"); t1Watch(upSab1, "up");
        t1Watch(downSab0, "down"); t1Watch(downSab1, "down");

        post({
          type: "init_success",
          data: {
            upstreamBuffers: [upSab0, upSab1],
            downstreamBuffers: [downSab0, downSab1],
            transportType: core.get_transport_type(),
          },
        });
        DEBUG && console.log("[CRDT Worker] init_success");
        break;
      }

      case "connect": {
        if (!core) throw new Error("EditorCore not initialized");
        // url 由主线程必传（同源相对，壳宿主代理）；worker 不持有任何主机配置。
        const url = data.url;
        if (!url) throw new Error("connect: 缺少 url");
        DEBUG && console.log("[CRDT Worker] connecting to", url);
        await core.connect(url);

        // 主事件循环（非阻塞）：LPPB 读 → CRDT 处理 → 网关收发
        // 引擎停机必须让宿主知道：只在 DEBUG 下 print ⇒ 生产态"死了和闲着一样"。
        core.run_loop().catch((err) => {
          DEBUG && console.error("[CRDT Worker] run_loop error:", err);
          post({ type: "error", error: "run_loop 停机: " + String((err && err.message) || err) });
        });

        post({ type: "connected", data: { url, transportType: core.get_transport_type() } });
        DEBUG && console.log("[CRDT Worker] connected + run_loop started");
        break;
      }

      case "update_upstream_buffer": {
        const { idx, sab } = data;
        updater?.update(idx, sab);
        t1Watch(sab, "up");   // T1：上游换缓冲 ⇒ 新 SAB 也要挂
        break;
      }

      case "get_telemetry": {
        // WASM 侧打点（WASM_local_proc / WASM_net_proc 等）→ 主线程 → gateway js-flush。
        // 独立导出函数（不碰 EditorCore 对象 —— run_loop 的 &mut 借用进行中，实例方法会 recursive use）
        const wIn = Date.now();
        const raw = editor_telemetry_data ? editor_telemetry_data() : "";
        console.log("[TELWK] wall=" + wIn + "->" + Date.now());   // worker 侧：收到请求 → 取数完成（经 console 桥到页面）
        post({ type: "telemetry_data", data: raw });
        // 镜像到 worker console（第 14-23 行的桥会转发到页面 console）⇒ 可被 CDP console 事件直接读到
        if (raw) console.log("[WTEL] " + String(raw).slice(0, 600));
        break;
      }

      case "dispose": {
        core = null;
        updater = null;
        post({ type: "disposed" });
        break;
      }
    }
  } catch (error) {
    DEBUG && console.error("[CRDT Worker] error:", error);
    post({ type: "error", error: error.message });
  }
};
