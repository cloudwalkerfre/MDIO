// @ts-check
/** 平台共享 shim：react-dom/client（createRoot/hydrateRoot —— renderer 本体）。 */
import ReactDOMClient from "react-dom/client";
export default ReactDOMClient;
export const { createRoot, hydrateRoot, version } = ReactDOMClient;
export * from "react-dom/client";   // 兜全集（同 react-dom.ts 的理由）
