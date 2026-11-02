// @ts-check
/**
 * snapshot —— 等某文档的**首个全量帧**（open 后 F 帧落地）。从 `demo/src/entry.client.ts` 搬来（S4）。
 * 纯函数式：client 由调用方注入（框架件不持有装配点的连接）。
 * @param {any} c 连接（client：subscribe(ev)）
 * @param {number} docId
 * @param {number} ms 超时毫秒
 * @returns {Promise<any>} 首帧的树
 */
export function waitSnapshot(c, docId, ms) {
  return new Promise((resolve, reject) => {
    const off = c.subscribe((ev) => {
      if (ev.docId === docId && ev.type === "snapshot") { clearTimeout(timer); off(); resolve(ev.tree); }
    });
    const timer = setTimeout(() => { off(); reject(new Error("文档首帧超时（docId=" + docId + "）")); }, ms);
  });
}
