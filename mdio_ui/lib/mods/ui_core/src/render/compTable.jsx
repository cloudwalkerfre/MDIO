// @ts-check
import { createElement, Fragment } from "react";
import { keyOf } from "./nodeKey.js";
import { modCompOf } from "../services/uiMd.js";

/**
 * compTable —— **组件方法的 router**（对应 /lp 讨论里的 ui.compFor(node)）：
 *   compFor(ui, node)：① 声明命中 → 该命名空间的 default（表序 = 优先级，见 uiMd.compileMatchTable）；
 *                      ② 未命中 → **声明里的默认渲染器（std）**；
 *                      ③ 再没有 ⇒ **null（不画）** —— 声明里没装渲染器是**声明的错**，不该由运行期兜。
 * 命中项**属于哪个桶**（node 逐节点 / section 区域封闭）由 uiStore.claimAt 给出，派发规则在 NodeView：
 * 两个桶都是"认领即自画"（node 桶的件只画自己的壳、孩子调 renderNode），只有**未认领**才由框架补画。
 * 换组件 = **换引用**：在声明侧加一条 claim 指向别的模块。
 * 注：router 放视图层（组件是视图对象）；store 只出 claimOf/modOf，快照保持纯数据。
 */
// 「内置件表 BlockDict」与「兜底渲染器 declaredDefault」**不存在**：
//   未认领 ⇒ 只交给**声明里的默认渲染器**（std，用 ui.modOf("std") 按名字取）；声明里没装 ⇒ 不画（返回 null）。
//   没有内置表、没有终极安全网、没有"永不白屏"的兜底件：装没装渲染器是**声明**的事，跑不通就该看得见。

// `Fallback`（朴素直出兜底件）**不存在**：
//   它原本是「永不白屏」的安全网 —— 而业主口径是**没有兜底**：没装渲染器就什么都不画，问题当场看得见。

export function compFor(ui, node, preClaim) {
  if (!ui || !node || typeof node !== "object") return null;   // 输入不合法 ⇒ 不画（不兜底）
  // **P4 收口**：判定由调用点（NodeView 的 per-node `computed`）**算一次**后传进来 ⇒ 这里不二次求值、也不兜。
  //   必传 `preClaim`：命中项对象或 `null`（明确「无认领」）—— 没有"不传就自己判一次"的兼容支（不存在）。
  const name = preClaim && preClaim.name;
  const rec = name ? ui.modOf(name) : null;     // 装载记录 { name, ns, url, via }：命名空间在 ns 上
  const Comp = modCompOf(rec);                  // "什么算能画"只有一处判据（与 uiStore.renderClaims 同源）
  if (Comp) return Comp;
  // ② 未认领 ⇒ 交给**声明里的默认渲染器**（std）：与「被认领」同一套渲染，视觉才一致。
  // ③ 声明里没装 ⇒ **null（不画）**：没有内置件表、没有兜底件 —— 这是声明的错，该在页面/诊断里看得见。
  return modCompOf(ui.modOf ? ui.modOf("std") : null) || null;
}

export default compFor;