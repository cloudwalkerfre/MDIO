// @ts-check
/**
 * ui_base/monaco —— 系统级 Monaco 供给（归属 ui_base，与 react/mst 同层）。
 *
 * - 版本/打包/CSS/worker 策略只在这里定；消费方（chat、devview mona 的 app 化、其它 wt app）
 *   统一 import 逻辑名 "ui_base/monaco"，由本包统一装配 Monaco。
 * - 入口用 editor.main：API + 全部语言（含 markdown）注册；当前只带 codicon CSS。
 * - worker 默认关：聊天输入/单文件编辑不需要语言服务 worker；需要时调用方可覆盖 MonacoEnvironment。
 */
import * as monaco from "monaco-editor/editor/editor.main";

/**
 * 惰性空 worker：只满足接口（postMessage/addEventListener/terminate），不真正起线程。
 * 为什么不能直接用 null：编辑器创建时 documentColors 等特性会经 editorWorkerService
 * 发消息，null 会抛 "Cannot read properties of null (reading 'postMessage')"。
 * 对聊天输入/单文件编辑，这些 worker 特性本就不需要，发出去的消息悬空即可。
 */
function createInertWorker() {
  return {
    postMessage: function () { /* no-op */ },
    addEventListener: function () { /* no-op */ },
    removeEventListener: function () { /* no-op */ },
    terminate: function () { /* no-op */ },
    dispatchEvent: function () { return true; },
    onmessage: null,
    onerror: null,
    onmessageerror: null
  };
}

/** 安装无真实 worker 的 MonacoEnvironment（幂等；调用方可在其后覆盖）。 */
export function installMonacoEnvironment() {
  try {
    const g = /** @type {any} */ (globalThis);
    if (!g.MonacoEnvironment || typeof g.MonacoEnvironment.getWorker !== "function") {
      g.MonacoEnvironment = { getWorker: function () { return createInertWorker(); } };
    }
  } catch (e) {
    /* 非浏览器环境忽略 */
  }
}

export { monaco };
export default monaco;
