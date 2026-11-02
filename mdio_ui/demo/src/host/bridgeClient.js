// Host-wasm transport for the MDIO browser client.
//
// This module is only reachable from entry.host.client.ts. The default web
// entry does not import it, so ordinary browser sessions keep the browser wasm
// transport and never load this code.
import { MdioClient, connectClient as connectBrowserClient } from "ui_core/client";

function hostBridgeConfig() {
  if (typeof location === "undefined") return null;
  try {
    const q = new URLSearchParams(location.search);
    const parent = q.get("hostBridge") === "parent" || globalThis.__MDIO_HOST_BRIDGE_PARENT__ === true;
    if (!parent && q.get("hostBridge") !== "1") return null;
    if (parent) return { parent: true, wtUid: String(q.get("wt") || "genesis") };
    return {
      endpoint: String(q.get("hostEndpoint") || ""),
      stream: String(q.get("hostStream") || ""),
      wtUid: String(q.get("wt") || "genesis"),
    };
  } catch (e) {
    return null;
  }
}

class HostMdioClient extends MdioClient {
  constructor(opts, host) {
    super(opts || {});
    this.host = host;
    this._hostSeq = 0;
    this._hostWaiters = new Map();
    this._hostParentBound = false;
    this._hostEs = null;
    // The base MdioClient gate for exec only checks writer presence. Host
    // transport has no SAB writer, but outbound frames go to _hostPost.
    this.writer = {};
  }

  async init() {
    this.isReady = true;
    this._startHostReadLoop();
    return { engine: "host-wasm", wtUid: this.host.wtUid };
  }

  async connect() {
    this._connState = "connected";
    return { mode: "host-wasm", wtUid: this.host.wtUid };
  }

  requestTelemetry() { /* no worker telemetry in host mode */ }
  probe() { return Promise.resolve(null); }

  _hostPost(body) {
    if (this.host.parent) {
      const id = ++this._hostSeq;
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          this._hostWaiters.delete(id);
          reject(new Error("host bridge timeout"));
        }, 15000);
        this._hostWaiters.set(id, { resolve, reject, timer });
        try {
          const payload = { ...body, wtUid: body.wtUid || this.host.wtUid };
          window.parent.postMessage({ type: "mdio-host:request", id, body: payload }, "*");
        } catch (e) {
          clearTimeout(timer);
          this._hostWaiters.delete(id);
          reject(e);
        }
      });
    }
    if (!this.host.endpoint) return Promise.reject(new Error("host bridge endpoint missing"));
    return fetch(this.host.endpoint, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ ...body, wtUid: body.wtUid || this.host.wtUid }),
      cache: "no-store",
    }).then(async (res) => {
      const data = await res.json().catch(() => ({}));
      if (!res.ok || !data.ok) throw new Error(data.error || ("host bridge HTTP " + res.status));
      return data;
    });
  }

  _hostFrame(peerId, docId, tag, payload) {
    const bytes = new Uint8Array(13 + payload.length);
    const dv = new DataView(bytes.buffer);
    dv.setUint8(0, tag);
    dv.setUint32(1, payload.length, true);
    dv.setUint32(5, peerId, true);
    dv.setUint32(9, docId, true);
    bytes.set(payload, 13);
    let binary = "";
    for (const b of bytes) binary += String.fromCharCode(b);
    return btoa(binary);
  }

  _installHostParentListeners() {
    if (this._hostParentBound) return;
    this._hostParentBound = true;
    window.addEventListener("message", (event) => {
      const data = event && event.data;
      if (!data || typeof data !== "object") return;
      if (data.type === "mdio-host:reply") {
        const waiter = this._hostWaiters.get(data.id);
        if (!waiter) return;
        this._hostWaiters.delete(data.id);
        clearTimeout(waiter.timer);
        if (data.ok) waiter.resolve(data.data);
        else waiter.reject(new Error(String(data.error || "host bridge error")));
        return;
      }
      if (data.type === "mdio-host:frame") {
        try {
          const binary = atob(String(data.data || ""));
          const bytes = new Uint8Array(binary.length);
          for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
          this._dispatchMessage(bytes);
        } catch (error) {
          console.error("[client] host frame parse failed:", error);
        }
      }
    });
  }

  _startHostReadLoop() {
    if (this.host.parent) {
      this._installHostParentListeners();
      return;
    }
    if (this._hostEs || !this.host.stream) return;
    const es = new EventSource(this.host.stream + "?wtUid=" + encodeURIComponent(this.host.wtUid));
    this._hostEs = es;
    es.onmessage = (event) => {
      try {
        const binary = atob(String(event.data || ""));
        const bytes = new Uint8Array(binary.length);
        for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
        this._dispatchMessage(bytes);
      } catch (error) {
        console.error("[client] host frame parse failed:", error);
      }
    };
    es.onerror = () => { this._connState = "disconnected"; };
  }

  sendPayload(peerId, docId, tag, payload) {
    if (this.onBeforeSend?.(docId, tag, payload)) return;
    const frame = this._hostFrame(peerId, docId, tag, payload);
    this._hostPost({ op: "send", frame }).catch((error) => {
      console.error("[client] host send failed:", error);
    });
  }

  async fullTree(wtUid) {
    const data = await this._hostPost({ op: "tree", wtUid: String(wtUid || this.host.wtUid) });
    return Array.isArray(data.tree) ? data.tree : [];
  }
}

/** Drop-in replacement for ui_core/client.connectClient used only by the host entry. */
export async function connectClient(url, opts) {
  const host = hostBridgeConfig();
  if (!host) return connectBrowserClient(url, opts);
  const client = new HostMdioClient(opts || {}, host);
  await client.init();
  await client.connect(url);
  return client;
}
