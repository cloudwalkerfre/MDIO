// @ts-check
import { createElement, memo, useCallback, useMemo } from "react";
import { observer } from "mobx-react-lite";
import { untracked } from "mobx";
import { getSnapshot } from "mobx-state-tree";
import { compFor } from "./compTable.jsx";
import { appRouter } from "../services/appRouter.js";   // 应用路由能力：**共享单例**（与入口件同一份；投递仍走 props）

import { isSectionShell, READ, matchOf, modCompOf } from "../services/uiMd.js";   // 拓扑表（唯一一份"什么算一节"）+ 读面（单源 READ）+ match trait 判定/取件
import { countExec } from "../services/monitor.js";   // 交付件执行计数（可信打点；判据 R6a 禁全局 ⇒ 走 monitor 通道）
import { keyOf } from "./nodeKey.js";

import ErrorBoundary from "./ErrorBoundary.jsx";
import { snapFor, tokenOf } from "./contentKey.js";   // 内容键：容器/混合拿窄键，其余拿子树快照（波 2）

/** 应用级恒定（单源 READ）⇒ 放进 props 不影响 memo。 */

/**
 * NodeView —— 渲染面（框架只 dispatch，别的都不做）。
 *   FunC  = compFor(ui, node)   // router：声明命中 → 声明里的默认渲染器（std）→ null（不画）
 *   claim = node.claim          // 命中项（**没有桶**；判定只有一条路 claimMatches → chainWalk（app 注入）；Node 上没有参 getter = computed）
 *
 * 派发规则（**由节点自身决定**，不由条目属性决定）：
 *   · **节（`isSectionShell(nt)` 即 `h`）被认领 ⇒ 封闭**：这一章怎么画由该件决定（画图 / 不画 / 调 renderNode
 *     复用节点渲染），框架不再往里派发 —— 这正是 sec 的设定：header 划出的章归认领件整章渲染。
 *   · **非节**：件只画**当前节点自己的壳**，孩子两种接法：
 *       ① 节（h/sec）的正文与子节，在 DOM 里是标题的**兄弟** ⇒ 框架接（SectionKids）；
 *       ② 其他元素的孩子装在元素**里面**（li 里 / ul 里 / blockquote 里）⇒ 只有件知道放哪
 *          ⇒ 件调 renderNode(child)（公开契约，std 与自定义件同名同形；不调用就永远不跑）。
 *   · **未认领**：交给**声明里的默认渲染器**（std）；声明里没装 ⇒ 不画（没有内置表、没有兜底件）。
 *
 *  四条实现约束（每条都有实测/出处，改动前先读）：
 *
 * 1) 内层函数**不能与导出的常量同名**。
 *    const NodeView = memo(observer(function NodeView(...) { ... <NodeView .../> ... }))
 *    中「具名函数表达式的名字在函数体内指向它自身」⇒ 递归渲染的是**裸组件**，memo/observer 全失效。
 *    实测：同名版 201 实例全渲、memo 比较器 0 次；改名版渲染 1 次、比较器 200 次全等。
 *    上游同题：mobx#3836 / MST#1718 / SO#70568283；回归测试 test/mst-recursion.mjs（9 项）。
 *
 * 2) 子节点 key 必须用**身份键**（nodeKey.js），不能用下标。
 *    实测（200 节插入一个新节）：下标 key ⇒ 202 实例重渲 / 233 次渲染；身份 key ⇒ 仅根 + 新节 / 33 次（↓86%）。
 *
 * 3) **列表要放在独立组件里**（MobX 官方 react-optimizations「Render lists in dedicated components」）。
 *    实测（真实 Chrome + React Profiler，同一构建 A/B）：
 *      · 根节内容编辑（200 节）：2.98ms → **0.29ms**；
 *      · 深层编辑（200 节）：3.17ms → **0.51ms**；（1000 节）17.14ms → **0.82ms**；
 *      · 结构插入 30 节：commit 累计 341ms → **112ms**（↓67%）。
 *
 * 4) 内容追踪三条（互补，第三条是本轮新增）：
 *    ① observer：读本节点子树的**内容版本**（MST 的 getSnapshot 是每节点缓存 computed ⇒ 未变返回同一对象）；
 *       未订阅 ⇒ 补丁丢失（实测：DOM 停在旧值、0 次渲染）。
 *    ② memo：父重渲时按 props 相等 bail（否则子节点被父级联带着重渲）。
 *    ③ **传给件的内容版本 snap**：框架把 getSnapshot(node) 一起交下去（本来就在算 —— 边界 resetKey 用的就是它），
 *       件照普通 React 写 useMemo(…, [snap]) 即正确更新。只传 node（MST 补丁不换实例 ⇒ 引用恒定）
 *       ⇒ 件里 useMemo([node]) 永不失效 ⇒ 重渲了但输出是陈的（E1 实测：三次重渲、零次 memo 执行）。
 *
 * 关于更细粒度（框架自动给 memo，不让每个 wt 自己包）：memoOf 按 (node, snap) 身份比较，零深比较、零成本。
 *
 * 组件契约：{ node, snap, exec, renderNode }（node/exec 是判官 R8 的老两样，snap/renderNode 是框架加的
 * 两样"自动项"）。std 保持纯组件（不 import mobx/MST）。
 */
/** 节判定统一读拓扑表。 */
const isSec = (n) => !!n && typeof n === "object" && isSectionShell(n.nt);

/**
 * **孩子通道**（`node.ct` 的原样数组；字符串是 array-like，不能当数组）。
 *  名字刻意**不叫 `kidsOf`**：链上的 `uiMd.kidsOf`/`read.kidsOf` 是**穿透外壳**的遍历（选择器要的），
 *    这里要的是"树面原样、逐节点交给 NodeView"。两者同名不同义，审计被点出来 ⇒ 改名分开。
 */
function childrenChannelOf(node) {
  return typeof node.ct !== "string" && node.ct && typeof node.ct.length === "number" ? Array.from(node.ct) : [];
}

/**
 * 节的**子节点列表**：独立组件（约束 3），按文档序逐个交给 NodeView。
 * 顺序 = 文档序（不是"先正文后子节"）：迁移后的声明里 \`### node\` / \`### sec\` 就是子节，
 * 位置必须按原文顺序，否则渲染与源文对不上。
 */
const SectionKids = memo(observer(function SectionKids({ node, ui, exec }) {
  return (
    <>
      {childrenChannelOf(node).map((c) => <NodeView key={"s" + keyOf(c)} node={c} ui={ui} exec={exec} />)}
    </>
  );
}));

/**
 * 框架自动给的 memo 边界（用户口径：该加 memo 的地方不吝惜，而且**由框架加**，不让每个 wt 自己包）。
 * 比较键 = props 身份：node（MST 补丁不换未变节点的实例）· snap（MST 每节点快照缓存 + structural
 * sharing ⇒ 未变即同一对象）· exec / renderNode（应用级恒定）。
 * ⇒ 内容没变的节点不重渲，变化只沿**路径**传播（对照：认领件一次自递归画整篇子树）。
 * 必须**按组件缓存**：memo 包装器每次渲染都新建 ⇒ element.type 换身份 ⇒ React 卸载重挂（不是我们想要的）。
 */
const memoCache = new WeakMap();
function memoOf(Comp) {
  // **空值/非对象不进缓存**：compFor 的既定语义是「没有渲染者 ⇒ null（不画）」，而 WeakMap 的键必须
  //   是对象 ⇒ 之前 `memoOf(null)` 直接 `WeakMap.set(null,…)` 抛 TypeError（实测：文档面无 app 时整片崩）。
  if (!Comp || (typeof Comp !== "function" && typeof Comp !== "object")) return Comp;
  let M = memoCache.get(Comp);
  if (!M) {
    // **组件执行计数**（可信打点规范 §一）：每次执行交付件就给 `ui_core/monitor` 的计数器 +1；
    //   探针在页面内 `await import("ui_core/monitor")` 读 `execSnapshot()`（判据 R6a 禁全局 ⇒ 不走全局）。
    //   为什么必须有它：`fiber.flags & PerformedWork` 会跨渲染**残留** ⇒ 计数虚高（实测报过假的「每帧 243×2 全量」），
    //   而 DOM 122/122 全存活；数**函数被执行了几次**才是精确口径。
    const Counted = function MdioCounted(props) {
      countExec();   // 一次整数自增；探针读 ui_core/monitor 的 execSnapshot()
      return createElement(Comp, props);
    };
    M = memo(Counted);
    memoCache.set(Comp, M);
  }
  return M;
}

/**
 * 下放渲染函数必须**应用级恒定**（业务件的 memo 四样里有它）：按「应用标识 + **状态值**」缓存闭包。
 * 状态是整数 ⇒ 相同状态值拿到**同一个**函数（真实声明下全文档不同状态数 ≤3）⇒ 业务件的跳过才成立；
 * 若每次渲染新建闭包 ⇒ memo 永不 bail ⇒ 逐层重渲。
 */
const tokenIds = new WeakMap();
let tokenSeq = 0;
function appToken(ui) {
  let t = tokenIds.get(ui);
  if (!t) { t = ++tokenSeq; tokenIds.set(ui, t); }
  return t;
}
const NodeViewBody = observer(function NodeViewBody({ node, ui, exec }) {
  // **判定 = `Node` 上的无参 getter（MST computed）**：`node.gates` 一条链 + 一次表扫，`claim`/`visible` 只是读它的结果。
  //   为什么从 `ui.gates(node)` 搬过来：官方 views 明文 ——**只有无参 getter 是 computed（显式缓存点）**，带参 view 零缓存；
  //   而且这些值只依赖「本节点 + 声明世代」⇒ 天然是按节点的缓存点（观察时缓存、卸载自动挂起）。
  // **读法必须是 tracked**（官方 react-integration：永远在 observer 内读 observable）；
  //   这里的 untracked 不依赖「每写必变」的计数器；
  //   实测：tracked 与 untracked 的扇出**完全相同**（`sop_bigdoc_fanout.mjs`：深层值写 Δ=1 / 1）。
  // [试点临时修复] 帧树当前以普通对象交付（无 MST views）⇒ claim/visible 走 ui.gates 兜底；MST 节点仍走视图。迁移完成后回直读。
  const _g = node && typeof node.claim !== "undefined" ? { claim: node.claim, visible: node.visible } : (ui ? ui.gates(node) : { claim: null, visible: true });
  const claim = _g.claim;
  const visible = _g.visible;
  if (!visible) return null;                            // 隐藏 ⇒ 整棵子树不画（无 vis 节时恒真）
  // （命中计数**已移出渲染路径**：诊断改由 `ui.claimHitsNow()` 离线 walk 数 —— 渲染路径无写，见 uiStore。）
  const FunC = untracked(() => compFor(ui, node, claim));   // router：声明命中 → 声明默认件 → null（不画）
  // **影响集（tracked）**：声明版本 + 当前文档路径。P3 之前这里还有一支「有不可局部化条目（Slice）时
  // 退化为订阅全局命中集钥匙」—— 判定全走路径 ⇒ 那一支不存在。
  //  这个读**必须在渲染体里真被读到**（不能只算不用）：上面的 comp/claim/visible 全是 untracked 读，
  //    唯一的唤醒源就是这里。曾因「只算不用」⇒ 装载表就绪后壳永不重算 ⇒ 整篇文档永久停在不画的状态
  //    （实测：/demos/README.md 渲染出 0 个 .mdio-std，DOM 只剩无 class 的裸 div/span）。
  const influence = ui ? (ui.declRev + ":" + ui.route) : "";
  // 内容键（三条用途同一个值：追踪信号 + 边界 resetKey + **交给件的更新键**）。
  // **波 2**：容器/混合 ⇒ 窄键（祖先名字链 + 本格 + 孩子身份；见 contentKey.js）；其余 ⇒ 子树快照。
  // 为什么能收窄：`getSnapshot(node)` 是 MST 的 computed，装配快照要读每个孩子的快照 ⇒ 读它 = **订阅整棵子树**；
  //   而「把子件交回框架」的容器只画自己那一格（它渲染的内容 ⊆ 窄键覆盖范围，判据 check_key_layers.mjs）。
  // 为什么窄键里要有**祖先名字链**：判定走 chainWalk（app 注入）(祖先链) ⇒ 祖先改名可能翻转命中/可见（R1.6）——读祖先的 t 是被追踪的。
  // **认领件例外**：它要整章渲染 ⇒ 必须拿宽键（子树快照），否则件里的 useMemo 拿到陈值（R1.10）。
  // [试点临时修复] 普通对象没有 MST getSnapshot ⇒ 退化返回自身。迁移完成后回 getSnapshot。
  const snap = snapFor(node, claim, (n) => { try { const s = getSnapshot(n); return s; } catch (e) { return n; } });
  // 公开契约 renderNode：把孩子交回框架渲染（框架再查认领/桶）。身份恒定（依赖是应用级常量）⇒ 上面的 memo 才有效。
  const renderNode = useCallback((n) => <NodeView key={"n" + keyOf(n)} node={n} ui={ui} exec={exec} />, [ui, exec]);
  const modMatch = useMemo(() => (name) => { const t = ui ? ui.modMatchOf(name) : null; return t ? (n) => matchOf(n, t) : null; }, [ui]);   // match trait 能力（使用点创建，与 appRouter 同法）
  // 取件面（trait 命中后按名字取件）：**返回组件函数**，与 compFor/renderClaims 同源（modCompOf 只认 default/Component）。
  // 曾返回装载记录（{name,ns,url,via} 对象）⇒ 命中后 h(记录) 触发 React #130（元素类型是 object）。
  const use = useMemo(() => (name) => (ui && name ? modCompOf(ui.modOf(name)) : null), [ui]);
  // **没有渲染者 ⇒ 什么都不画**（compFor 的第三支）。位置在两处：① 所有 hooks 之后（hooks 顺序恒定）；
  //   ② 上面 `influence` 的 tracked 读之后（装载表就绪/声明换代时这里仍会被唤醒重算，不会永久停在「不画」）。
  if (!FunC) return null;
  const Comp = memoOf(FunC);
  // 认领件外包一层崩溃隔离：单个组件炸掉只降级成该节点内的错误卡，文档其余部分继续渲染
  //（旧壳只有面板级边界 ⇒ 一个坏组件会把整篇文档变成错误卡）。
  // 交付形状 = { node, exec, snap, renderNode, read }（**框架内部件一律不给业务件**；读面是唯一的读侧入口）。
  const body = (
    <ErrorBoundary name={"节点 " + String(node.nt)} resetKey={tokenOf(snap) + "|" + influence}>   {/* R1.1：snap 可能是对象 ⇒ 必须过 tokenOf（原来 snap + 串恒为 [object Object]） */}
      <Comp node={node} exec={exec} snap={snap} renderNode={renderNode} read={READ} appRouter={appRouter} modMatch={modMatch} use={use} />
    </ErrorBoundary>
  );
  if (!isSec(node)) return body;
  // 节的孩子由谁接：**认领即自画**（两个桶同一条规矩 —— node 桶的件只画自己的壳、孩子自己调 renderNode；
  // section 桶命中即封闭）。只有**未认领**才由框架补画：std 自己的节件递归子节，
  // 不补画则节内容整块消失（实测：/.mds/ui.md 只渲染出三个标题）。
  if (claim) return body;
  return (
    <>
      {body}
      <SectionKids node={node} ui={ui} exec={exec} />
    </>
  );
});

const NodeView = memo(NodeViewBody);
NodeView.displayName = "NodeView";

export default NodeView;
