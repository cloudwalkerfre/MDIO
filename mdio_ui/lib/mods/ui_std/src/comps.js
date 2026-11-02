// @ts-check
/**
 * **`std/comps`** —— 基础 md 渲染件（只管「给一个节点 / 给一篇文档，画出来」）。
 *
 * 边界（不许越）：**不许认领**（谁画哪段）· **不许**决定整篇布局 · **不许**带 app 生命周期 —— 那些是 app 的事（`std/basic_render`）。
 * 谁都能用这一层：别的 app · demo 框架自身 · router 的 default fallback（要「渲染一篇 md」时取它，**不装 app** ⇒ 单 wt 单 app 不破）。
 *
 * **对外组件归本层**：
 *   · `MDRoot` = 「**渲染 md 根节点 / 整篇文档体**」的组件（= 原语 `renderNode()` 的组件壳）；
 *   · `MDNode` = 「渲染**单个节点**」的组件（= 原语 `renderNode(node)` 的组件壳）。
 *   框架侧只留**原语工厂**（`ui_core/render/DocPane` 的 `makeRenderNode`）与开发面兜底屏。
 */
export { default as Std } from "./std.js";

/** 渲染**单个节点**（原语 `renderNode(node)` 的组件壳）。 */
export function MDNode(props) {
  const rn = props && props.renderNode;
  const n = props && props.node;
  if (typeof rn !== "function" || !n) return null;
  return rn(n);
}

/** 渲染 **md 根节点 / 整篇文档体**（原语 `renderNode()` 的组件壳）—— 「要一篇 md」时取的就是它。 */
export function MDRoot(props) {
  const rn = props && props.renderNode;
  return typeof rn === "function" ? rn() : null;
}

export default MDRoot;