// @ts-check
/** 平台共享 shim：react/jsx-runtime（JSX 自动运行时也要同一份）。 */
import JSXRuntime from "react/jsx-runtime";
export const { jsx, jsxs, Fragment } = JSXRuntime;
export default JSXRuntime;
