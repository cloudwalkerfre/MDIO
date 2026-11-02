// @ts-check
/**
 * ui_wasm/worker —— worker 工厂（web 目标知识归适配器）。
 * vite 识别 new Worker(new URL(...)) 模式：构建期把 ./worker.js 打成独立 chunk 并重写 URL。
 * 装配点（demo/entry.client）注入 createWorker；client.js 不 import 任何层内模块。
 */
export function createWorker() {
  return new Worker(new URL("./worker.js", import.meta.url), { type: "module" });
}
