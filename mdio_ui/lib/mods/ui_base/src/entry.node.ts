// @ts-check
/**
 * 平台供给包（provider）的行为面。
 *
 * 职责边界（重要）：
 *  - **构建期**：本包 provides 了 react/react-dom/mobx/mobx-react-lite/mobx-state-tree
 *    ⇒ 任何 \`uses: ["ui_base"]\` 的模块在构建时会把它们**自动外部化**（esmx 合并供给表推导），
 *    于是模块产物里保留裸 \`import "react"\`，而不是自带一份 React。
 *  - **运行期**：裸 specifier 由页面 import map 解析 —— 指向**宿主那一份**实例（宿主 entry.node.ts 的
 *    withSharedReact 注入）。所以 renderer 与所有模块共用同一份 React，hooks 的 dispatcher 才成立。
 *
 * 本文件不需要 server / postBuild；devApp 只要能产出 ESM 出口即可（无 JSX，故不挂 react 插件）。
 */
export default {
  root: undefined,
  modules: { links: {} },
  async devApp(esmx: unknown) {
    const { createViteApp } = await import("@esmx/vite");
    return createViteApp(esmx as never, { minimize: false });
  },
};
