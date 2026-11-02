// @ts-check
/** 平台供给包的 server 入口桩（同 client：只供给，不渲染）。 */
export const platform = { provides: ["react", "react-dom", "mobx", "mobx-react-lite", "mobx-state-tree"] };
export default platform;
