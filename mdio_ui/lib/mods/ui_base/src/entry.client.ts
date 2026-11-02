// @ts-check
/** 平台供给包（provider）的 client 入口桩：本模块不渲染任何东西，只**供给**平台包（react/mobx 全家）。 */
import "../style.css";   // 平台样式层（tailwind 本体）：import 即 esmx 联邦 CSS 的触发点 ⇒ manifest css[] ⇒ host rc.css() 注入 <link>（官方「共享样式」机制）
export const platform = { provides: ["react", "react-dom", "mobx", "mobx-react-lite", "mobx-state-tree"] };
export default platform;
