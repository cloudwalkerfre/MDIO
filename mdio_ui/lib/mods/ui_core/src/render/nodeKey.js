// @ts-check
/**
 * 节点身份键（React key 用）—— 与文本/下标无关：插入/删除/改名都不换存活项的身份。
 *
 * 为什么不能用下标当 key（实测，见 .tmp/sop/RENDER-TRACKING-OPTIONS.md §20）：
 *   在 200 节文档里插入一个新节：下标 key ⇒ 其后所有兄弟的 key 不变但 node 变了
 *   ⇒ memo 全部失效 ⇒ **202 个实例重渲、233 次渲染**；
 *   换成身份键 ⇒ 只有**根 + 新节**两个实例重渲（33 次，↓86%），其余靠 React 移动复用。
 * 老壳同一约定：DocRenderer.jsx:133「身份轴（R3/R4）：key = κ(node)，与文本/下标无关 ⇒
 * 改名不换 key、插入不换存活项的 key」。
 *
 * 实现：按**节点实例**发号（WeakMap）——MST 补丁不换未变节点的实例，
 * 新节点自然拿到新号；节点被替换（F 帧换代）时旧号随实例回收。
 */
const idOf = new WeakMap();
let seq = 0;

export function keyOf(node) {
  if (!node || (typeof node !== "object" && typeof node !== "function")) return "n";
  let k = idOf.get(node);
  if (k === undefined) {
    seq += 1;
    k = seq;
    idOf.set(node, k);
  }
  return k;
}

export default keyOf;
