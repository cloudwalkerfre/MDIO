// @ts-check
/** 平台共享 shim：react-dom。 */
import ReactDOM from "react-dom";
export default ReactDOM;
export const { createPortal, flushSync, version, findDOMNode, preconnect, prefetchDNS, preinit, preload, unstable_batchedUpdates } = ReactDOM;
// 兜全集：生产里真包已被**强制内联**进本 shim（见 devApp 的 external 谓词包装）⇒ `export *` 不会自引用；
// 只解构固定清单会漏掉清单外的名字（实测 `unstable_batchedUpdates`），在生产里报 does not provide an export。
export * from "react-dom";
