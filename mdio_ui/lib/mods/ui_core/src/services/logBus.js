// @ts-check
/**
 * logBus —— **管线日志的数据源**（环形缓冲 + 订阅/快照）。
 *
 * 从 ui_devview/devkit 迁到 ui_core（业主口径：log 是服务，不是壳的呈现关切；
 * devview 槽位改造阶段收口）。消费者：devview 外壳、exp_log app、任何需要看管线日志的件。
 * 接线在装配点：monitorSink(log)；WS 事件由 monitor.watch(client) 播报。
 *
 * 纪律：无全局探针、无网络；组件经 useSyncExternalStore 读。
 */

const MAX = 1000;   // 旧壳 debugLog 的环缓冲口径

/** @type {{at: string, text: string}[]} */
let entries = [];
/** @type {Set<() => void>} */
const listeners = new Set();

// 快照必须是稳定引用（useSyncExternalStore 的要求）
let snapshot = { entries };

/** [HH:MM:SS.mmm]（旧壳 debugLog 的时间戳格式）。 */
function stamp() {
  const d = new Date();
  const p = (n, w) => String(n).padStart(w, "0");
  return p(d.getHours(), 2) + ":" + p(d.getMinutes(), 2) + ":" + p(d.getSeconds(), 2) + "." + p(d.getMilliseconds(), 3);
}

export function log(text) {
  entries.push({ at: stamp(), text: String(text) });
  if (entries.length > MAX) entries.splice(0, entries.length - MAX);
  // 快照换新对象即可让订阅者重渲；数组原地追加（每补丁 3-4 行日志，不复制环缓冲）。
  snapshot = { entries };
  for (const cb of Array.from(listeners)) { try { cb(); } catch (e) { /* ignore */ } }
}

export function logSubscribe(cb) {
  listeners.add(cb);
  return function () { listeners.delete(cb); };
}

export function logSnapshot() {
  return snapshot;
}
/** 清空日志（旧壳 LogPanel 的「清空日志」按钮）。 */
export function logClear() {
  entries = [];
  snapshot = { entries: [] };
  for (const cb of Array.from(listeners)) { try { cb(); } catch (e) { /* ignore */ } }
}
