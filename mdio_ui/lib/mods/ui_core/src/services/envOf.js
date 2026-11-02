// @ts-check
/**
 * envOf —— 统一取工具面（方案 3 定案 · 落点 ui_core）。
 *
 * env 只装工具（函数与服务）；MST 节点一律不进 —— EnvTools 类型把这条纪律焊死。
 * 基础键：exec（命令通道）/ read（读面）/ appRouter（页内路由）/ client（连接服务）/ getMainRoot（主树取数桥，app 店独立成树用）。
 * app 可动态扩展自己的工具函数（如 compileRoutes），但只许函数与服务。
 * 静默容错：未注入时返回空工具面（app 在 dev 面独立运行是常态；不 console.warn —— 收口已定不加告警）。
 */
import { getEnv } from "mobx-state-tree";

/**
 * EnvTools —— env 工具面的形状（基础键 + 可扩展；全部是函数/服务对象，不含 MST 节点）。
 * @typedef {{
 *   exec?: Function,
 *   read?: Object,
 *   appRouter?: Object,
 *   client?: Object,
 *   getMainRoot?: Function,
 *   [k: string]: any,
 * }} EnvTools
 */

/** 取当前节点的工具面（env）；无 env 时退空对象（静默）。 */
export function envOf(self) {
  try { return getEnv(self) || {}; } catch (e) { return {}; }
}
