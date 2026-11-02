// @ts-check
import { observer } from "mobx-react-lite";
import NodeView from "./NodeView.jsx";
import { keyOf } from "./nodeKey.js";
/**
 * DocPane —— 文档面：树根的子节点逐个交给 NodeView。
 * observer：读 ui.doc 与 ui.doc.ct（可观察数组）被追踪 —— 顶层增删子节点即重渲；
 * 深层内容变化由各 NodeView 自己的「本节点快照版本」负责，所以这里不再依赖全局 rev。
 * key 用**身份键**（nodeKey.js）：插入/删除兄弟不换存活项的 key（下标 key 会让全部兄弟重渲，实测见 NodeView 头注）。
 */
const DocPane = observer(function DocPane({ ui, exec }) {
  const root = ui && ui.doc;
  // [试点临时修复] 帧树当前以普通对象交付（ct=真数组、无 MST kids 视图）⇒ 优先 ct，MST kids 兜底；迁移完成后回 `root.kids`。
  const kids = root ? (Array.isArray(root.ct) ? root.ct : (root.kids || [])) : [];
  // **文档体 = 一个 MD 排版作用域**（按业主口径「顶头加不加 header 两种渲染」修正）：
  //   std 只在 h/sec（isSection）上包 `.mdio-std`，其余节点直接出元素 ⇒ **无标题文档**（首块就是列表/引用）
  //   的正文不在 `.mdio-std` 里 ⇒ appearance.css 的排版（font-size/line-height/color/标题与列表的间距等）
  //   整片不生效 ⇒ 与「顶头有 h1」的同一篇文档渲染不同。这里由**外壳**补上作用域：文档体本来就是一段渲染好的 MD。
  //   有 h/sec 时各节自己也会包一层 ⇒ 外层与内层同值（继承）不出可见差异。
  const body = kids.map((n) => <NodeView key={"c" + keyOf(n)} node={n} ui={ui} exec={exec} />);
  return (
    <div className="mdio-doc mdio-std">
      {body}
    </div>
  );
});

export default DocPane;
/**
 * **原语工厂（B1 真搬）**：给宿主造 `renderNode` ——
 *   `renderNode()` = **整篇文档体**；`renderNode(node)` = **那个节点**（逐节点派发/认领照旧）。
 * 对外的**组件**归 `std/comps`（`MDRoot` / `MDNode`）；框架这里只出**原语**，不再导出组件（`NodeSlot` 不存在）。
 */
export function makeRenderNode(ui, exec) {
  return (node) => (node ? <NodeView node={node} ui={ui} exec={exec} /> : <DocPane ui={ui} exec={exec} />);
}
