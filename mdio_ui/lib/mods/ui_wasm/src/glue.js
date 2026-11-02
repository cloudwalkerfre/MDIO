// @ts-check
/**
 * ui_wasm/glue —— wasm-bindgen 胶水的薄再导出。
 * 引擎本体不属 esmx：这里不改胶水一字，只把 web 目标要的面摆出来。
 * 胶水自带的加载形态 = new URL('editor_core_bg.wasm', import.meta.url) ⇒ vite 构建期重写为哈希资产 URL。
 */
export { default as init, initSync, EditorCore, LppUpdater, editor_telemetry_data } from "./wasm/editor_core.js";
