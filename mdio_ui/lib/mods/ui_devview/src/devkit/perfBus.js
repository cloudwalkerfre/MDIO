// @ts-check
import { log as busLog } from "ui_core/logBus";
/**
 * perfBus —— 管线性能的**单一数据源**（PerfBox 与 LogPanel 共用同一份 fps 与分段）。
 *
 * 口径沿用老壳 PerfBox：三段自洽
 *   总时长 = incr（写发出 → 补丁到达） + render（补丁 → 渲染完成）
 * 打点来自 DevView 的帧订阅（与日志里 patch_latency / recv_to_render 同源，不是另起一套计时）。
 * 新鲜窗口 5s：过期后 PerfBox 只显示 fps（避免展示陈旧数字）。
 */
let subs = new Set();
let seq = 0;
// last = 最近一次补丁帧；lastWrite = 最近一次**由写触发的**会话（一次写常来多个补丁帧，
// 后面的帧没有写标记 ⇒ 只记 last 会把 incr 覆盖成空）。展示用 lastWrite 优先。
let state = { v: 0, fps: 0, frames: { f: 0, p: 0 }, last: null, lastWrite: null };

const nowMs = () => (typeof performance !== "undefined" && performance.now ? performance.now() : Date.now());

// ── 可信时间口径（见 `worktrees/genesis/wt/可信打点规范.md` §8）────────────────────────────
// 为什么要有：按「按键后固定等 N 秒」量的 Task/Script 抖动极大（实测 133–188ms）—— 窗口里混了
// Monaco / 日志 / 后台任务 ⇒ 只能当噪声。可信做法两条，都落在这里（本文件属 ui_devview ⇒ 只在 dev）：
//   ① **交互窗口**用 mark/measure 框住：`perfMarkRender` 打 `mdio:commit`；探针在按键前打 `mdio:key`，
//      两者之差 = 「按键 → 渲染提交」的真实窗口（不含等待、不含后台任务）。
//   ② **主线程阻塞**用 longtask 观测（统计只存在本模块内，经 perfSnapshot 显示在 PerfBox —— 判据 R6a
//      禁止全局探针，所以任何东西都不挂到 window 上；探针从面板文本读）。

function emit() {
  state = { ...state, v: (seq += 1) };
  for (const cb of Array.from(subs)) { try { cb(); } catch (e) { /* 订阅者异常不影响打点 */ } }
}

export const perfSubscribe = (cb) => { subs.add(cb); return () => { subs.delete(cb); }; };
export const perfSnapshot = () => state;

// ── 主线程阻塞：longtask 观测（≥50ms 的任务）────────────────────────────────────────
// 不挂全局（判据 R6a）⇒ 统计存在本模块内，PerfBox 显示；探针从面板文本读。
let long = { n: 0, totalMs: 0, maxMs: 0 };
export const perfLongSnapshot = () => long;
try {
  const PO = globalThis.PerformanceObserver;
  if (PO && PO.supportedEntryTypes && PO.supportedEntryTypes.indexOf("longtask") >= 0) {
    new PO((list) => {
      for (const e of list.getEntries()) {
        long = { n: long.n + 1, totalMs: long.totalMs + e.duration, maxMs: Math.max(long.maxMs, e.duration) };
        try { busLog("LONG-FRAME: " + Math.round(e.duration) + "ms（主线程长任务，累计 " + long.n + " 次）"); } catch (err) { /* ignore */ }
      }
      emit();
    }).observe({ entryTypes: ["longtask"] });
  }
} catch (e) { /* 环境不支持 ⇒ 面板不显示 long 行 */ }

/** fps 由 PerfBox 的 rAF 循环写入（单点采样，LogPanel 只读）。 */
export function perfSetFps(n) {
  if (state.fps === n) return;
  state = { ...state, fps: n };
  emit();
}

/** 帧计数：全量帧（F 帧）。 */
export function perfCountFullFrame() {
  state = { ...state, frames: { ...state.frames, f: state.frames.f + 1 } };
  emit();
}

/** 帧计数：补丁帧（P 帧）。 */
export function perfCountPatchFrame() {
  state = { ...state, frames: { ...state.frames, p: state.frames.p + 1 } };
  emit();
}

/** 补丁到达：定出 incr 段，并清空 render 段（等 DOM 提交后补上）。
 *  incr 口径（业主）：**信息进入 wasm → PJ 出 wasm**，与触发类型无关
 *  （Monaco MC_UP / exec / 模式帧 …… 都算）。差值由 client 挂在帧事件的 `ev.wasmMs` 上；
 *  缺失时退化为「写锚 → 补丁到达」。 */
export function perfMarkPatch(ops = 0, bytes = 0, wasmMs = null) {
  const t = nowMs();
  // 只认 wasm 窗口：没有本地发送（例如别人写的帧）⇒ incr 为空，这是正确结果，不是缺失
  const incrMs = (typeof wasmMs === "number" && isFinite(wasmMs)) ? wasmMs : null;
  const sess = { incrMs, patchAt: t, renderAt: null, ops: Number(ops) || 0, bytes: Number(bytes) || 0 };
  state = { ...state, last: sess, lastWrite: incrMs != null ? sess : state.lastWrite };
  emit();
}

/** 渲染完成（**DOM 提交完毕**；不再用 rAF —— 那会把「等下一帧」算进 render）。 */
export function perfMarkRender() {
  const t = nowMs();
  try { performance.mark("mdio:commit"); } catch (e) { /* 标记失败不影响功能 */ }
  if (!state.last) return;
  const isWriteSession = !!state.lastWrite && state.lastWrite.patchAt === state.last.patchAt;
  state = {
    ...state,
    last: { ...state.last, renderAt: t },
    lastWrite: isWriteSession ? { ...state.lastWrite, renderAt: t } : state.lastWrite,
  };
  emit();
}