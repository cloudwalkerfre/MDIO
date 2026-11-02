// @ts-check
/**
 * claimEval —— **「认领与可见性」的求值口子**（框架侧只留口子，策略归 app）。
 *
 * 【为什么】跟读单 B2：「顶层 `## match`/`## visibility` 取消；**认领与可见性归 `basic_render`**」。
 *   框架保留的：能力（`apply`）+ 判定谓词（`claimMatches`：路径门控 + 候选面 + 链上命中）+ 表数据 + 派发。
 *   搬去 app 的：**策略** —— 表尾往前扫谁赢、exclude 优先/无 include 全可见 这类规则。
 *
 * 【装配】宿主在挂上被指名的 appview 时调用 `setClaimEval(ns.createClaimEval(read))`
 *   （app 契约新增一项**可选导出**；缺席 ⇒ 口子保持空，认领不了，**不猜不兜**）。
 *   必须在**首次渲染之前**装好：`Node.get gates` 是 MST computed，装晚了早先算出的结果会留在缓存里。
 *
 * 零 import（services 层纪律 R1）。
 */
let evalFn = null;
/** @param {((store:any, node:any, chain:any[], parent:any) => {claim:any, visible:boolean})|null} fn */
export function setClaimEval(fn) { evalFn = typeof fn === "function" ? fn : null; }
export function getClaimEval() { return evalFn; }
export function clearClaimEval() { evalFn = null; }
