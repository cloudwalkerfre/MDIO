// @ts-check
/**
 * contentKey —— 「**这个节点自己画多大**」的内容键（波 2）。
 *
 * 为什么需要它：`getSnapshot(node)` 是**整棵子树**的 JSON（MST 的 computed，装配快照要读每个孩子的快照）
 *   ⇒ 在 `observer` 的渲染里读它 = **订阅整棵子树** ⇒ 深层一改，祖先链依次失效（O(深度)）。
 *   而「把子件交回框架」的容器**只画自己那一格** ⇒ 它不需要订整棵子树。
 *
 * 三条类别（依据 = std 的实际画法；`.tmp/p0/check_key_layers.mjs` 的漂移检查盯着这几行）：
 *   ① 纯容器（`h` / `bl` / `od` / `bq` / `admonition` / `vmr_container`）
 *      ⇒ 键 = 祖先名字链 + 本格 + **孩子身份序列**（std.js:363/380/384/386；节壳由框架 SectionKids 补画）
 *   ② 混合（`b` / `x` / `ol`）：`p` 与非块孩子**就地画**、块孩子交回（std.js:310 的 BLOCK_CHILD · :350 三岔）
 *      ⇒ 键 = 祖先名字链 + 本格 + **就地画者的内容** + 交回者的**身份**
 *   ③ 其余（`p` / 行内 / `tb` / `tr` / `c`/`ic` / 未知 nt…）⇒ **子树快照**（保持不动，安全侧）
 *
 * 为什么键里要 **祖先名字链**：判定（认领 / 可见）走 `chainWalk(ir, 祖先链)`（app 注入） ⇒ 祖先改名可能翻转本节点的命中；
 *   旧的宽快照顺带覆盖了这件事，收窄后必须显式带上（R1.6）。读祖先的 `t` 是**被追踪**的 ⇒ 祖先改名会唤醒本件 ✓。
 *
 * 返回值是**字符串**（①/②）或**快照对象**（③）：字符串按值比较 ⇒ 组件 memo 与 `resetKey` 都安全。
 */
import { getParent, hasParent, isAlive, getType, getPropertyMembers } from "mobx-state-tree";
import { asArray } from "../services/uiMd.js";
import { keyOf } from "./nodeKey.js";

/** ① 纯容器（**白名单**：只放已核对过 std 分派的 nt；拿不准一律落 ③ 的宽键 = 安全侧）。 */
const PURE = new Set(["h", "bl", "od", "bq", "admonition", "vmr_container"]);
/** ② 混合（就地画 + 交回）。 */
const MIXED = new Set(["b", "x", "ol"]);
/** std.js:310 的 BLOCK_CHILD（判据守漂移）。 */
const BLOCK_CHILD = new Set(["p", "h", "b", "x", "ol", "bl", "od", "c", "ic", "bq", "tb", "hb", "hr", "cm", "he"]);
/** 本格字段：**从模型反射取**（`getPropertyMembers(getType(node)).properties`），只排除下层通道 `ct`。
 *  为什么不写死清单：① 交付 JSON 增键时模型声明了就自动进键；② 判官 R6c（前端无拓扑概念）不许在源码里出现
 *  拓扑词汇 —— 写死清单会连着根节点那两个键一起写进来。反射取法两者都避开了。 */
const ownKeysOf = (node) => {
  const props = (getPropertyMembers(getType(node)) || {}).properties || {};
  const out = [];
  for (const k of Object.keys(props)) if (k !== "ct") out.push(k);
  return out;
};

export const isPureContainer = (node) => !!node && PURE.has(node.nt);
export const isMixed = (node) => !!node && MIXED.has(node.nt);
export const BLOCK_CHILD_SET = BLOCK_CHILD;

const kidsOf = (n) => asArray(n && n.ct);
const rawOf = (v) => (v === undefined || v === null ? "" : typeof v === "object" ? JSON.stringify(v) : String(v));
const ownOf = (n) => ownKeysOf(n).map((k) => k + "=" + rawOf(n[k])).join(",");

/** 对象 → 稳定 token（同一个对象恒同串）；原始值原样返回。给 `resetKey` 用（对象/字符串都安全）。
 *  **复用 `nodeKey.js` 的 `keyOf`**（同一个 WeakMap 发号器）：不新开第二个身份表 —— 收口判据（check_no_new_machinery）
 *  立刻抓到了第一版自带的 `tokenIds`，这里按「一处所有者」改回复用。 */
export function tokenOf(v) {
  return v === null || typeof v !== "object" ? String(v) : "o" + keyOf(v);
}

/** 祖先名字链（位置 = 祖先路径在名字面上的投影；判定用同一条链）。 */
function ancestorNames(node) {
  const out = [];
  let cur = node;
  let guard = 0;
  while (cur && isAlive(cur) && guard < 512) {
    guard += 1;
    if (!hasParent(cur)) break;          // 根：链到此为止（正常终止，不是兜底）
    const up1 = getParent(cur);          // 一跳 = 数组/映射包装
    if (!up1) break;
    const up2 = getParent(up1);          // 两跳 = 模型父
    if (!up2) break;
    // 文档边界（与 Node.chain 同一条规则）：出了文档树（Wt/Wts/UiStore 不是节点、没有 nt）就停 ——
    // 主树加深后（node→md→wt→wts→UiStore），不带这条会把容器层当祖先，最后在根上 getParent 抛「Failed to find the parent」。
    if (typeof up2.nt !== "string") break;
    out.push(rawOf(up2.t) + (up2.nt ? ":" + up2.nt : ""));
    cur = up2;
  }
  out.reverse();
  return out.join("/");
}

/**
 * 内容键：纯容器 / 混合 ⇒ **字符串**；其余 ⇒ **子树快照对象**（调用方只当「变没变」用）。
 */
export function contentKeyOf(node) {
  if (!node || typeof node !== "object") return String(node);
  const nt = node.nt;
  const head = "anc:" + ancestorNames(node) + "|own:" + ownOf(node);
  if (PURE.has(nt)) {
    return head + "|ids:" + kidsOf(node).map((c) => keyOf(c)).join(",");
  }
  if (MIXED.has(nt)) {
    const ks = kidsOf(node);
    const inline = ks.filter((c) => c && (c.nt === "p" || !BLOCK_CHILD.has(c.nt))).map((c) => rawOf(c)).join(";");
    const blockIds = ks.filter((c) => c && c.nt !== "p" && BLOCK_CHILD.has(c.nt)).map((c) => keyOf(c)).join(",");
    return head + "|inline:" + inline + "|blockids:" + blockIds;
  }
  return null;   // ③：由调用方退回子树快照
}

/**
 * 调用方一行接入（**两个维度**）：
 *   · **认领件**（`claim` 非空）⇒ **宽键 = 子树快照**：它要**整章**渲染（note §2：sec 件拿 `node/snap`），
 *     键必须覆盖它读到的一切（R1.10：memo deps 不覆盖 ⇒ 重渲了但拿到陈值）。
 *   · **未认领** ⇒ 按类别：① 纯容器 / ② 混合 ⇒ 窄键字符串；③ 其余 ⇒ 子树快照。
 */
export function snapFor(node, claim, getSnapshot) {
  if (claim) return getSnapshot(node);
  const k = contentKeyOf(node);
  return k === null ? getSnapshot(node) : k;
}

