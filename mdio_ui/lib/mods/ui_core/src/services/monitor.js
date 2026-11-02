// @ts-check
/**
 * monitor —— **监控出口**（emit-only，无缓冲、无历史）。
 *
 * 位置（业主口径："高度围绕 ws，少部分围绕渲染，建议靠拢 => core"）：
 *   · 它住在 `client.js`（WS 对讲机）**旁边** —— 监控的主体就是那条连接的事件流；
 *   · 采集/缓冲/面板**不在这里**：缓冲在 `ui_core/services/logBus.js`（服务层，从 devview 迁入），面板是 app；
 *   · 装配点负责接线：`monitorSink(log)` 把面板接上出口；没人接就落 console（产品面无 devview 也能看到）。
 *
 * 两块内容：
 *   ① **WS 侧（主体）**：`watch(client)` —— 订阅连接状态与文件树事件并播报；装配点连上之后调一次。
 *   ② **渲染侧（少数）**：`report(text)` —— 框架件主动播报（ErrorBoundary 降级、CSS 采纳/卸载）。
 *
 * 纪律（与 services 层一致）：不 import 其它 service（判据 R1）；不碰 DOM；不读装配点的全局。
 */

/** @type {(text: string) => void} */
let sink = (text) => { try { console.warn("[mdio] " + text); } catch (e) { /* 无 console 也不许崩 */ } };

/** 播报一条事件（框架侧唯一入口）。
 *  @param {string} text */
export function report(text) {
  try { sink(String(text)); } catch (e) { /* 播报失败不拖垮调用方 */ }
}

/**
 * **交付件执行计数**（可信打点规范 §一「组件执行计数」；判据 R6a 禁止全局探针 ⇒ 走这条已导出的通道）。
 *   NodeView 每执行一次交付件就 +1；探针在页面内 `await import("ui_core/monitor")` 后读 `execSnapshot()`
 *   （与 vis / error-boundary 套件读模块的同款做法）。计数本身只是一次整数自增，零成本。
 */
let execCount = 0;
export function countExec() { execCount += 1; }
export function execSnapshot() { return execCount; }

/** 接上收集器（装配点用它把 devview 的日志面板接上）。传非函数 = 恢复默认 console 出口。
 *  @param {(text: string) => void} fn */
export function monitorSink(fn) {
  sink = typeof fn === "function" ? fn : (text) => { try { console.warn("[mdio] " + text); } catch (e) { /* ignore */ } };
}

/**
 * **订阅 WS 连接的事件流**（① 主体）：包住装配点已经设好的回调（不夺权，先播报再转发）。
 *   · 连接状态 ⇒ `boot: conn <state>`（cursor 面板/CdpDevview 都靠它判断连接已就绪）
 *   · 文件树   ⇒ `tree: entries <n>`（原始计数；**带 wt 作用域与过滤的那一条**由宿主/装配点自己播报，
 *              因为"排掉别的 wt 的挂载树"是宿主侧策略，不是连接层事实）
 * @param {any} c 连接（client）
 * @returns {() => void} 退订（把回调还原）
 */
export function watch(c) {
  if (!c) return () => {};
  const prevConn = c.onConnectionState;
  const prevTree = c.onTree;
  c.onConnectionState = (s) => {
    report("boot: conn " + s);
    try { if (typeof prevConn === "function") prevConn(s); } catch (e) { /* ignore */ }
  };
  c.onTree = (entries) => {
    report("tree: entries " + (Array.isArray(entries) ? entries.length : 0));
    try { if (typeof prevTree === "function") prevTree(entries); } catch (e) { /* ignore */ }
  };
  return () => { c.onConnectionState = prevConn; c.onTree = prevTree; };
}
