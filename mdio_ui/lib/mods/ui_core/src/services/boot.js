// @ts-check
/**
 * boot —— 层1 服务：进程生命周期状态机。
 *
 * 只有三个状态：booting / ready / failed。
 * 各层把自己的启动步注册进来（registerStep），由 main.jsx（装配点）统一装配并调用 start()。
 * 任何一步抛错 => failed，错误原文原样进错误卡；没有静默降级，没有自动重试。
 *
 * 纪律：本文件不 import 任何其他层（判官：boot.js 的 import 行数必须为 0）。
 */

/** @type {"booting" | "ready" | "failed"} */
let state = "booting";

/** @type {{ step: string, message: string, at: string } | null} */
let error = null;

/** @type {{ name: string, fn: () => void | Promise<void> }[]} */
const steps = [];

/** @type {Set<(s: typeof state, e: typeof error) => void>} */
const listeners = new Set();

let started = false;

// 快照必须是稳定引用：useSyncExternalStore 的 getSnapshot 每次返回新对象会死循环
let snapshot = { state, error };

/** 注册一个启动步。只能在 start() 之前调用。 */
export function registerStep(name, fn) {
  if (started) throw new Error("boot: 启动已开始，不允许再注册步骤（" + name + "）");
  if (typeof name !== "string" || !name.trim()) throw new Error("boot: 步骤名必须是字符串");
  if (typeof fn !== "function") throw new Error("boot: 步骤 " + name + " 不是函数");
  if (steps.some((s) => s.name === name)) throw new Error("boot: 步骤名重复：" + name);
  steps.push({ name, fn });
}

/** 状态快照（稳定引用，只读）。 */
export function getState() {
  return snapshot;
}

/** 订阅状态变迁；返回退订函数。 */
export function subscribe(cb) {
  listeners.add(cb);
  return function () { listeners.delete(cb); };
}

function emit() {
  snapshot = { state, error };
  for (const cb of Array.from(listeners)) {
    try { cb(state, error); } catch (e) { console.error("boot: 监听器抛错", e); }
  }
}

/** 按注册序跑完所有步骤。幂等：第二次调用直接返回当前状态。 */
/**
 * **外部判失败**：给"启动超时"这类**不由步骤抛错**的失败一个入口。
 * 为什么需要：装载卡住时（宿主/网关未就绪、产物缺失）步骤既不抛错也不返回 ⇒ 页面永远停在"正在装载…"，
 * 用户看不到原因、也没有重试入口。调用它 ⇒ 走与步骤失败**同一条**路径（同一个错误卡 + 重试按钮）。
 * @param {string} step 步骤名（约定：超时用 "bootTimeout"）
 * @param {string} message 给人看的原因
 */
export function fail(step, message) {
  if (state === "failed") return;
  state = "failed";
  error = { step, message: String(message || "启动超时"), at: new Date().toISOString() };
  try { console.error("boot: 外部判失败 →", step, message); } catch (e) { /* ignore */ }
  emit();
}

/**
 * **重置以支持重入**（换树不刷新的基础）：`start()` 有一次性守卫（`started`），
 * 换 wt 时必须能**再跑一遍**步骤序列（client → uiMd）而不刷新页面。
 * 只清"运行态"（started/state/error），**不清步骤注册**（步骤由各装配点 import 时注册一次）。
 * @returns {boolean} 是否真的重置了（已在装载中则拒绝，避免并发两次 start）
 */
export function reset() {
  if (started && state === "booting") return false;
  started = false;
  state = "booting";
  error = null;
  emit();
  return true;
}

export async function start() {
  if (started) return snapshot;
  started = true;
  for (const s of steps) {
    try {
      await s.fn();
    } catch (e) {
      state = "failed";
      error = {
        step: s.name,
        message: String((e && e.message) ? e.message : e),
        at: new Date().toISOString(),
      };
      console.error("boot: 步骤失败 →", s.name, e);
      emit();
      return snapshot;
    }
  }
  state = "ready";
  emit();
  return snapshot;
}
