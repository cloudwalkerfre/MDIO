// @ts-check
import { installGuards, armWatchdog } from "../test/lib/watchdog.mjs";
installGuards("check-admit-gate");
armWatchdog("check-admit-gate", 10000);   /* = 10s；实测 0.2s（扫描 1 跑）⇒ ×2 */
/**
 * check-admit-gate —— **协议层准入门（R1）的活体判据**：裸 ws 直连网关，不依赖浏览器。
 *
 * 判的是什么（符号名定位，行号随并发编辑漂移）：`mdio_core/src/store.rs` 的
 * `Store::handle_packet(packet, conn_wt)` 在 `admit_gate_on()`（env `MDIO_ADMIT_GATE`，
 * 默认 "1"；"0" ⇒ 负对照，跳过门）为真时，对 UP_OPEN(0x02)/UP_SYNC(0x04) 先跑
 * `doc_reachable_from(conn_wt, doc_id)`；不可达 ⇒ 立刻回 **DnErr(0xFF)**（理由文本
 * "doc=N 不在连接 wt=X 的可达集"），不再进 `open_session`。判定式现状 =
 * ① meta.wt_uid == conn_wt ∨ ② conn_wt 的后代闭包按各自 storage_root 前缀匹配 meta.storage_key
 * ∨ ③ conn_wt 不在树上 ⇒ true；meta 缺失 ⇒ false。
 *
 * 连接面：`mdio_py/src/mdio_py/gateway/handlers/multiplexer.py` 的 `/ws/gateway/{wt_uid}`
 * ——URL 里的 wt 就是 `conn_wt`；老端点 `/ws/gateway`（无 wt）传 None ⇒ 不设门（本判据不测它）。
 * 帧头 13 字节：Tag(1) + Len(4) + Peer(4) + Doc(4)，全小端（`mdio_core/src/protocol.rs`）。
 * doc_id 由 HTTP `/fs/tree?wt_uid=<wt>&hidden=<bool>`（`gateway/api/fs.py`，
 * 同一 `refresh_tree_json`）**现场取**，不写死——doc_id 每次网关重启重新分配。
 *
 * 断言集（**写死**；门修好前应红、修好后全绿）：
 *   ① 合法 genesis doc 放行（conn=genesis）
 *   ② 他 wt 的 doc 必须回 0xFF（conn=挂载树，doc 归属 genesis）——夹具先自验：
 *      该 doc 必须在连接 wt 的（含隐藏）树里**找不到**，否则"他 wt"名不副实 ⇒ 报夹具错
 *   ③ 不存在的 doc 必须回 0xFF（meta 缺失 ⇒ 与旧 Python 版"不在可达集"同判）
 *   ④ 未知 conn_wt（`nonsense`）**必须回 0xFF**——**行为回退**（不是新增严格性）：旧 Python
 *      `_reachable_doc_ids` 对未知 wt 取树不抛异常、得空集 ⇒ 拒绝；"判不了 ⇒ 接受"只在取树抛异常时。
 *      现行 Rust「取不到节点 ⇒ true」相对旧语义是放宽，本任务恢复旧语义。
 *      **同时是堵漏**（红队实测）：现行该分支放行时 `DnOpen` **载荷含正文**——本判据 ④ 自己的原始帧里
 *      就能看到 "j# Chat: demo …" 正文 ⇒ 未知 conn_wt 的放行是**内容泄露**，不止是语义偏差。
 *   ⑤ 挂载树（默认 c8Page01）自己的 doc（`/CHAT_HIS.md`）放行——**当前复现拒绝的那一条**
 *   ⑥ genesis 正常：一次 UpTree(0x01) 的第 1 条 DnTree(0x81) 帧 == HTTP(genesis, hidden=true) 树
 *   ⑦ boot 声明放行：挂载树隐藏树里的 `/.mds/ui.md`（= 客户端 boot 的 UI_MD 步真正打开的那一件）放行——
 *      业主"页面打不开"的**直接信号**（声明取不到 ⇒ uiMd · 文档首帧超时）；夹具缺失时显式报缺，不静默放过
 *   ⑧ A2 视图归属：挂载视图与根视图是**两份树**，同一物理件可能**同 id 共用 arena 槽**（实测
 *      c8Page01 `/CHAT_HIS.md`=109 同时出现在 genesis 树 `/.mds/mounts/mnt_c8Page01/CHAT_HIS.md`）
 *      ⇒ 共享 id 必须**两个连接都放行**；而**根视图独占** id（genesis `/CHAT_HIS.md`=115）在挂载连接上
 *      被拒是**正确**的（"另一视图的 id"= 客户端帧混用，**不算门 bug**）。本项把两者分开判，防止误判。
 *   ⑨ A3 带 rule 的 binding 遮蔽格：**跳过并标注**（当前不可触发，需要 rule）——见行内理由。
 *   ⑩ A4 顺序不变量：新连接上 **UP_TREE 必须先于首个 UP_OPEN** 且真的登记了 id。取一棵本次
 *      **从未用 HTTP 取过树**的 wt（HTTP `/fs/tree` 自己就会登记 meta，先取过就判不出来），
 *      同一连接"先收树帧 → 立刻按该帧的 id 开"。
 *   ⑪ A5 死 id 过放（文件已删、arena 槽未清，现状门 ① `meta.wt_uid == conn_wt` 会放行死 id）：
 *      **跳过并标注**（代码级结论，活体今天无样本：genesis 视图 id 1..228 零缺口）。
 *
 * 用法：node scripts/check-admit-gate.mjs [--wt c8Page01] [--strict]
 *   env：MDIO_GATEWAY（HTTP 基址，默认 http://127.0.0.1:8011）
 *        MDIO_WT（= --wt；连接 wt / 挂载树，默认 c8Page01）
 *        MDIO_ADMIT_TIMEOUT（每条探针硬超时 ms，默认 6000）
 *        MDIO_ADMIT_EVIDENCE（原始响应落盘，默认 /tmp/check-admit-gate.evidence.json）
 *   --strict：网关/夹具缺失时不再"⊘ 跳过 + 退 0"，而是退 1（防止把"没跑到"当"全绿"）。
 * 依赖：网关活着（默认 8011）。不碰浏览器 / CDP；只连 WS + 读 HTTP 树，不改服务端状态
 *（`get_vfs_tree` 只读 `&WtTree`，未知 wt 不会因此注册进树 ⇒ ④ 可反复跑）。
 */
import fs from "node:fs";

const GW = (process.env.MDIO_GATEWAY || "http://127.0.0.1:8011").replace(/\/+$/, "");
const WS_BASE = GW.replace(/^http/, "ws");
const GENESIS = "genesis";
const argWt = process.argv.indexOf("--wt");
const MOUNT_WT = String((argWt >= 0 ? process.argv[argWt + 1] : "") || process.env.MDIO_WT || "c8Page01");
const STRICT = process.argv.includes("--strict");
const PROBE_MS = Math.max(1000, Number(process.env.MDIO_ADMIT_TIMEOUT || 6000));
const EVIDENCE = process.env.MDIO_ADMIT_EVIDENCE || "/tmp/check-admit-gate.evidence.json";
const HEADER = 13;
const PEER = 0xa11d;               // 非零 peer：回复必须回填同一值（0x82 分支用 packet[5..9]）
const ABSENT_DOC = 0xfffffff0;     // ③ 用一个不可能被分配的 doc id（arena 自增，永远到不了）

const TAG_NAME = { 0x01: "UpTree", 0x02: "UpOpen", 0x04: "UpSync", 0x81: "DnTree", 0x82: "DnOpen", 0x83: "DnSync", 0x84: "DnAst", 0x85: "DnExec", 0x86: "DnFile", 0xff: "DnErr" };
const tagName = (t) => TAG_NAME[t] || ("0x" + Number(t).toString(16).padStart(2, "0"));
const hex = (t) => "0x" + Number(t).toString(16).padStart(2, "0").toUpperCase();
const preview = (s) => String(s).replace(/\s+/g, " ").slice(0, 120);

/** 13 字节帧头（Tag+Len+Peer+Doc，小端）+ 空载荷。 */
function frame(tag, doc, peer = PEER) {
  const b = new Uint8Array(HEADER);
  b[0] = tag;
  const dv = new DataView(b.buffer);
  dv.setUint32(1, 0, true);        // Len = 载荷长度 = 0
  dv.setUint32(5, peer, true);
  dv.setUint32(9, doc >>> 0, true);
  return b;
}

/** 树取证：返回 [{path, id, hash}]（与页面 client.fullTree 同一条 URL，见 check-tree-source）。 */
async function treeOf(wt, hidden) {
  const url = GW + "/fs/tree?hidden=" + (hidden ? "true" : "false") + "&wt_uid=" + encodeURIComponent(wt);
  const r = await fetch(url, { signal: AbortSignal.timeout(8000) });
  if (!r.ok) throw new Error("HTTP " + r.status + " " + url);
  const j = await r.json();
  if (!Array.isArray(j)) throw new Error("响应不是数组：" + url);
  return j;
}

async function statusOf() {
  const r = await fetch(GW + "/control/status", { signal: AbortSignal.timeout(5000) });
  if (!r.ok) throw new Error("HTTP " + r.status);
  return await r.json();
}
let ST = null;                       // /control/status 缓存（一轮探针里只取一次；探针本身不改状态）
const status = async () => (ST = ST || await statusOf());

/**
 * 裸 ws 探针：连上照 ws 规矩**先挂 message 监听再发帧**（`open` 事件里才 send），
 * 收到 wantTags 里的第一帧即收工；条件轮询 + 硬超时双保险（`--strict` 时超时按红）。
 */
function ask(wt, bytes, wantTags, ms = PROBE_MS) {
  return new Promise((resolve) => {
    const rec = { wt, url: WS_BASE + "/ws/gateway/" + encodeURIComponent(wt), sentTag: hex(bytes[0]), doc: new DataView(bytes.buffer).getUint32(9, true), ms: 0, how: "", frames: [] };
    const t0 = Date.now();
    let ws = null;
    let settled = false;
    let hard = null;
    let openT = null;
    const finish = (how) => {
      if (settled) return;
      settled = true;
      clearTimeout(hard); clearTimeout(openT);
      rec.how = how; rec.ms = Date.now() - t0;
      try { if (ws) ws.close(); } catch (e) { /* 忽略 */ }
      resolve(rec);
    };
    try { ws = new WebSocket(rec.url); } catch (e) { rec.how = "constructor: " + String((e && e.message) || e).slice(0, 60); rec.ms = Date.now() - t0; return resolve(rec); }
    ws.binaryType = "arraybuffer";
    hard = setTimeout(() => finish("hard-timeout"), ms + 2000);
    openT = setTimeout(() => finish("open-timeout"), 5000);
    ws.addEventListener("message", (ev) => {
      const b = Buffer.from(ev.data);
      const tag = b.length ? b[0] : -1;
      const f = { tag, name: tagName(tag), len: b.length >= HEADER ? b.readUInt32LE(1) : -1, peer: b.length >= HEADER ? b.readUInt32LE(5) : -1, doc: b.length >= HEADER ? b.readUInt32LE(9) : -1, bytes: b.length, text: "", raw: null };
      // raw 只为结构断言留内存（证据落盘时剥掉：0x81 树帧可达数十 KB）
      if (b.length > HEADER) { const txt = b.subarray(HEADER).toString("utf8"); f.text = preview(txt); f.raw = txt; }
      rec.frames.push(f);
      if (wantTags.indexOf(tag) >= 0) finish("answered");
    });
    ws.addEventListener("error", () => finish("ws-error"));
    ws.addEventListener("close", (ev) => finish("closed:" + (ev && ev.code)));
    ws.addEventListener("open", () => { clearTimeout(openT); try { ws.send(bytes); } catch (e) { finish("send-error: " + String((e && e.message) || e).slice(0, 60)); } });
  });
}

/**
 * 同一连接上的**顺序**探针（A4）：open → 发 steps[i].bytes → 等其 wantTags → 发下一帧 …
 * steps[i].bytes 可为函数 (frames) => Uint8Array（下一帧要用上一帧结果时，如「先树帧、再开那个 id」）。
 * 同样是先挂监听再发帧、条件轮询 + 硬超时。
 */
function askSeq(wt, steps, ms = PROBE_MS) {
  return new Promise((resolve) => {
    const rec = { wt, url: WS_BASE + "/ws/gateway/" + encodeURIComponent(wt), ms: 0, how: "", frames: [], sent: [] };
    const t0 = Date.now();
    let ws = null;
    let settled = false;
    let i = 0;
    let hard = null;
    let openT = null;
    const finish = (how) => {
      if (settled) return;
      settled = true;
      clearTimeout(hard); clearTimeout(openT);
      rec.how = how; rec.ms = Date.now() - t0;
      try { if (ws) ws.close(); } catch (e) { /* 忽略 */ }
      resolve(rec);
    };
    const sendNext = () => {
      if (i >= steps.length) return finish("answered");
      let bytes = steps[i].bytes;
      if (typeof bytes === "function") { try { bytes = bytes(rec.frames); } catch (e) { return finish("step-error"); } }
      if (!bytes) return finish("step-no-frame");
      rec.sent.push({ i, tag: hex(bytes[0]), doc: new DataView(bytes.buffer).getUint32(9, true) });
      try { ws.send(bytes); } catch (e) { return finish("send-error"); }
    };
    try { ws = new WebSocket(rec.url); } catch (e) { rec.how = "constructor"; rec.ms = Date.now() - t0; return resolve(rec); }
    ws.binaryType = "arraybuffer";
    hard = setTimeout(() => finish("hard-timeout"), ms + 2000);
    openT = setTimeout(() => finish("open-timeout"), 5000);
    ws.addEventListener("message", (ev) => {
      const b = Buffer.from(ev.data);
      const tag = b.length ? b[0] : -1;
      const f = { tag, name: tagName(tag), len: b.length >= HEADER ? b.readUInt32LE(1) : -1, peer: b.length >= HEADER ? b.readUInt32LE(5) : -1, doc: b.length >= HEADER ? b.readUInt32LE(9) : -1, bytes: b.length, text: "", raw: null };
      if (b.length > HEADER) { const txt = b.subarray(HEADER).toString("utf8"); f.text = preview(txt); f.raw = txt; }
      rec.frames.push(f);
      if (i < steps.length && steps[i].wantTags.indexOf(tag) >= 0) { i += 1; sendNext(); }
    });
    ws.addEventListener("error", () => finish("ws-error"));
    ws.addEventListener("close", (ev) => finish("closed:" + (ev && ev.code)));
    ws.addEventListener("open", () => { clearTimeout(openT); sendNext(); });
  });
}

/** 首帧摘要（逐条断言与证据都印它）。 */
const shown = (probe) => {
  if (!probe.frames.length) return "无响应帧（" + probe.how + "，" + probe.ms + "ms）";
  const f = probe.frames[0];
  return f.name + "(" + hex(f.tag) + ") len=" + f.len + " doc=" + f.doc + " peer=" + f.peer
    + (f.text ? ' " ' + f.text + ' "' : "") + " · " + probe.how + " " + probe.ms + "ms";
};

const rows = [];
const step = (no, name, ok, detail) => {
  rows.push({ no, name, ok, detail });
  console.log("  " + no + " " + (ok ? "PASS" : "FAIL") + "  " + name + (detail ? "\n         → " + detail : ""));
};
/** 明确标注的「跳过并标注」：不计红绿（``--strict`` 也不把它算红），但必须在输出与证据里留理由。 */
const skipStep = (no, name, why) => {
  rows.push({ no, name, ok: true, skip: true, detail: why });
  console.log("  " + no + " SKIP  " + name + (why ? "\n         → " + why : ""));
};
const note = (s) => console.log("      · " + s);
const idsOf = (t) => t.map((e) => Number(e.id)).sort((a, b) => a - b);
const pathsOf = (t) => t.map((e) => String(e.path)).sort();
const same = (a, b) => a.length === b.length && a.every((v, i) => v === b[i]);
const eqTree = (a, b) => !!a && !!b && same(idsOf(a), idsOf(b)) && same(pathsOf(a), pathsOf(b));
/** 把 0x81 帧的载荷解析成树数组（解析不了返回 null；不抛）。 */
const parseTreeFrame = (f) => {
  if (!f || !f.raw) return null;
  try { const a = JSON.parse(f.raw); return Array.isArray(a) ? a : null; } catch (e) { return null; }
};

console.log("check-admit-gate: 网关 " + GW + " · 挂载树 " + MOUNT_WT + " · 默认门（不设 MDIO_ADMIT_GATE）· 每条探针 " + PROBE_MS + "ms");

// ── 闸门：网关不在 ⇒ 本判据无从谈起（照 check-tree-source 约定：⊘ 跳过 + 退 0；--strict 退 1）
let genesisPlain = null;
let genesisHidden = null;
try {
  genesisPlain = await treeOf(GENESIS, false);
  genesisHidden = await treeOf(GENESIS, true);
} catch (e) {
  const why = String((e && e.message) || e).slice(0, 100);
  console.log("⊘ 跳过：网关取不到树（" + GW + "/fs/tree）——" + why);
  console.log(STRICT ? "✗ --strict：网关不在 ⇒ 退 1（不把\"没跑到\"当\"全绿\"）" : "  （网关起来后重跑：node scripts/check-admit-gate.mjs）");
  process.exit(STRICT ? 1 : 0);
}

// ── 夹具：挂载树（不存在就退到 status.wts 里第一棵非空、非 genesis 的树，并说明）
let mountPlain = null;
let mountHidden = null;
let mountNote = "";
try {
  mountHidden = await treeOf(MOUNT_WT, true);
  mountPlain = await treeOf(MOUNT_WT, false);
} catch (e) { mountHidden = null; mountPlain = null; }
if (!mountHidden || !mountPlain || !mountPlain.length) {
  const st = await statusOf();
  const cands = (Array.isArray(st && st.wts) ? st.wts : []).filter((u) => u && u !== GENESIS);
  for (const u of cands) {
    try {
      const p = await treeOf(u, false);
      if (!p.length) continue;
      mountPlain = p; mountHidden = await treeOf(u, true);
      mountNote = "（--wt " + MOUNT_WT + " 取不到树 ⇒ 退到 " + u + "）";
      break;
    } catch (e) { /* 换下一棵 */ }
  }
}
if (!mountPlain) {
  console.log("⊘ 跳过：没有可用的挂载树夹具（" + MOUNT_WT + " 取不到，且 status.wts 里也没有非空非 genesis 的树）");
  process.exit(STRICT ? 1 : 0);
}
const usedMountWt = mountNote ? /退到 (\S+?)）/.exec(mountNote)[1] : MOUNT_WT;

// 夹具选取：按 path 找，找不到就退首条（树里必有件，且都在 UI 文件树里可见）
const pick = (tree, prefer) => tree.find((e) => String(e.path) === prefer) || tree[0] || null;
const gChat = pick(genesisPlain, "/CHAT_HIS.md");            // ①
const gHand = pick(genesisPlain, "/HANDOVER.md");            // ② "他 wt 的 doc"
const mChat = pick(mountPlain, "/CHAT_HIS.md");              // ⑤
const mDecl = mountHidden.find((e) => String(e.path) === "/.mds/ui.md") || null;   // ⑦ 客户端 boot 开的那一件

console.log("  夹具" + mountNote + "：genesis 树 " + genesisPlain.length + " 条（hidden " + genesisHidden.length + "）· "
  + usedMountWt + " 树 " + mountPlain.length + " 条（hidden " + mountHidden.length + "）");
note("① genesis /CHAT_HIS.md id=" + (gChat ? gChat.id : "?") + " · ② 他 wt 候选 genesis /HANDOVER.md id=" + (gHand ? gHand.id : "?")
  + " · ⑤ " + usedMountWt + " /CHAT_HIS.md id=" + (mChat ? mChat.id : "?") + " · boot 声明 /.mds/ui.md id=" + (mDecl ? mDecl.id : "?"));

const evidence = { at: new Date().toISOString(), gateway: GW, mountWt: usedMountWt, genesisWt: GENESIS, fixtures: { gChat, gHand, mChat, mDecl }, probes: {}, rows: null };

// ── ① 合法 genesis doc 放行
let p1 = await ask(GENESIS, frame(0x02, gChat ? gChat.id : 0), [0x82, 0xff]);
evidence.probes["①"] = p1;
step("①", "合法 genesis doc 放行（conn=" + GENESIS + " doc=" + (gChat ? gChat.path + "#" + gChat.id : "?") + "）",
  p1.frames.length > 0 && p1.frames[0].tag === 0x82, shown(p1) + " · 期望 DnOpen(0x82)");

// ── ② 他 wt 的 doc 必须回 0xFF
// 夹具两条自检（缺一条这条断言就名不副实）：(a) 该 doc id 不得出现在连接 wt 的树里（含隐藏）；
// (b) 它的归属 wt 不得是连接 wt 的**后代**（`conn_wt ∈ chain(归属wt)`）——后代在 B3 语义下本就可达，
// 拿它当"他 wt"会把"该拒"错判成"该放"。首选 genesis 的 /HANDOVER.md；自检不过就换别棵 wt 的件。
const inMountTree = (d) => !!d && mountHidden.some((e) => Number(e.id) === Number(d.id));
const chainOf = async (u) => {
  try {
    const r = await fetch(GW + "/control/wt/chain?uid=" + encodeURIComponent(u), { signal: AbortSignal.timeout(5000) });
    const j = await r.json();
    return Array.isArray(j && j.chain) ? j.chain.map(String) : [u];
  } catch (e) { return [u]; }
};
let otherDoc = gHand;
let otherOwner = GENESIS;
let fixtureWhy = "";
if (!inMountTree(otherDoc)) {
  fixtureWhy = "genesis " + (otherDoc ? otherDoc.path + "#" + otherDoc.id : "（树为空）");
} else {
  fixtureWhy = "**夹具缺失**：genesis " + otherDoc.path + " 在 " + usedMountWt + " 树里可见";
  const st = await status();
  for (const u of (Array.isArray(st && st.wts) ? st.wts : []).filter((x) => x && x !== usedMountWt)) {
    if ((await chainOf(u)).indexOf(usedMountWt) >= 0) continue;      // u 是连接 wt 的后代 ⇒ 不能当"他 wt"
    let t = null;
    try { t = await treeOf(u, false); } catch (e) { continue; }
    const hit = t.find((d) => !inMountTree(d));
    if (hit) { otherDoc = hit; otherOwner = u; fixtureWhy = u + " " + hit.path + "#" + hit.id + "（原 genesis 夹具不可用 ⇒ 换用）"; break; }
  }
}
const fixtureBad = fixtureWhy.indexOf("**夹具缺失**") === 0;
let p2 = fixtureBad ? { frames: [], how: "fixture-invalid", ms: 0 } : await ask(usedMountWt, frame(0x02, otherDoc ? otherDoc.id : 0), [0x82, 0xff]);
evidence.probes["②"] = p2;
step("②", "他 wt 的 doc 必须回 0xFF（conn=" + usedMountWt + " doc=" + (otherDoc ? otherDoc.path + "#" + otherDoc.id : "?") + " 归属 " + otherOwner + "）",
  !fixtureBad && p2.frames.length > 0 && p2.frames[0].tag === 0xff,
  "夹具：" + fixtureWhy + " · " + shown(p2) + " · 期望 DnErr(0xFF)");

// ── ③ 不存在的 doc 必须回 0xFF
let p3 = await ask(GENESIS, frame(0x02, ABSENT_DOC), [0x82, 0xff]);
evidence.probes["③"] = p3;
step("③", "不存在的 doc 必须回 0xFF（conn=" + GENESIS + " doc=" + ABSENT_DOC + "）",
  p3.frames.length > 0 && p3.frames[0].tag === 0xff, shown(p3) + " · 期望 DnErr(0xFF)");

// ── ④ 未知 conn_wt 必须回 0xFF（**行为回退**，不是新增严格性）
// 旧 Python 语义（参考实现 crdt.py:_reachable_doc_ids，读码核实）：对未知 wt 取树**不抛异常**、
// 得**空集**（store.rs refresh_tree_json：collect_vfs_paths 在 tree.nodes 里取不到 uid 就 return，
// 于是 paths 为空 ⇒ 返回 []）⇒ "doc 不在可达集" ⇒ **拒绝**。"判不了 ⇒ 接受"只发生在取树**抛异常**
// 时（_reachable_doc_ids 的 except → None）。现行 Rust 在未知 wt 上返回 true（取不到节点 ⇒ 判不了 ⇒ 接受）
// 相对旧语义是**放宽**，本任务按「回退错误的部分」恢复旧语义 ⇒ 未知 conn_wt 必须 0xFF、不进 open_session。
// **同时是堵漏**（红队实测）：现行放行时回复的 DnOpen `载荷里带正文`——本项自己的原始帧就看得见
// "j# Chat: demo …" 的内容 ⇒ 未知 conn_wt 的放行是**内容泄露**，改判 0xFF 不只是语义回退。
let p4 = await ask("nonsense", frame(0x02, gChat ? gChat.id : 0), [0x82, 0xff]);
evidence.probes["④"] = p4;
step("④", "未知 conn_wt=nonsense 必须回 0xFF（doc=" + (gChat ? gChat.path + "#" + gChat.id : "?") + " 归属 " + GENESIS + "）",
  p4.frames.length > 0 && p4.frames[0].tag === 0xff,
  shown(p4) + " · 期望 DnErr(0xFF)（与旧 Python 同判：未知 wt 取树得空集 ⇒ 拒绝）；0x82 = 未知 wt 被放宽");

// ── ⑤ 挂载树自己的 doc 放行（当前的红灯）
let p5 = await ask(usedMountWt, frame(0x02, mChat ? mChat.id : 0), [0x82, 0xff]);
evidence.probes["⑤"] = p5;
step("⑤", "挂载树自己的 doc 放行（conn=" + usedMountWt + " doc=" + (mChat ? mChat.path + "#" + mChat.id : "?") + "）",
  p5.frames.length > 0 && p5.frames[0].tag === 0x82, shown(p5) + " · 期望 DnOpen(0x82)");

// ── ⑥ genesis 正常：第 1 条树帧 == HTTP(genesis, hidden=true)
let p6 = await ask(GENESIS, frame(0x01, 0), [0x81]);
evidence.probes["⑥"] = p6;
let t6 = null;
if (p6.frames.length && p6.frames[0].tag === 0x81 && p6.frames[0].raw) {
  try { t6 = JSON.parse(p6.frames[0].raw); } catch (e) { t6 = null; }
}
const okTree6 = !!t6 && Array.isArray(t6) && eqTree(t6, genesisHidden);
step("⑥", "genesis 正常：UpTree 第 1 条 DnTree == HTTP(" + GENESIS + ", hidden=true) 树",
  p6.frames.length > 0 && p6.frames[0].tag === 0x81 && okTree6,
  shown(p6) + " · 帧 " + (t6 && Array.isArray(t6) ? t6.length : "?") + " 条 vs HTTP " + genesisHidden.length + " 条（id/path 逐项）");

// ── ⑦ boot 声明放行：客户端 UI_MD 步真正打开的那一件（挂载树的 /.mds/ui.md）
// 业主症状链的最后一环：声明 doc 被门拒 ⇒ uiMd 取不到声明 ⇒ 文档首帧超时。夹具缺失（隐藏树里没有该件）
// 说明这棵 wt 根本 boot 不起来 ⇒ 显式报缺 FAIL，不当"没跑到"跳过。
let p7 = mDecl ? await ask(usedMountWt, frame(0x02, mDecl.id), [0x82, 0xff]) : { frames: [], how: "fixture-missing", ms: 0 };
evidence.probes["⑦"] = p7;
step("⑦", "boot 声明放行（conn=" + usedMountWt + " doc=" + (mDecl ? mDecl.path + "#" + mDecl.id : "/.mds/ui.md 不在隐藏树里") + "）",
  !!mDecl && p7.frames.length > 0 && p7.frames[0].tag === 0x82,
  (mDecl ? "" : "**夹具缺失**：隐藏树里没有 /.mds/ui.md（UI_MD 步将取不到声明）· ")
  + shown(p7) + " · 期望 DnOpen(0x82)（业主「打不开」的直接信号）");

// ── ⑧ A2 视图归属：同一物理件在两份树里可能**同 id**（共用 arena 槽）⇒ 两个连接都必须放行；
// 而**另一视图独占**的 id 在挂载连接上被拒是**正确**的（客户端帧混用，不算门 bug）——分开判，防误判。
const hasId = (tree, id) => tree.some((e) => Number(e.id) === Number(id));
const sharedIds = mountHidden.map((e) => Number(e.id)).filter((id) => hasId(genesisHidden, id));
const sharedPick = [];
for (const pref of ["/CHAT_HIS.md", "/.mds/ui.md"]) {
  const hit = mountHidden.find((e) => String(e.path) === pref);
  if (hit && sharedIds.indexOf(Number(hit.id)) >= 0 && !sharedPick.some((x) => Number(x.id) === Number(hit.id))) sharedPick.push(hit);
}
const a2RootOnly = (gChat && !hasId(mountHidden, gChat.id)) ? gChat : (genesisPlain.find((e) => !hasId(mountHidden, e.id)) || null);
const a2 = [];
let a2i = 0;
for (const d of sharedPick) {
  for (const wt of [usedMountWt, GENESIS]) {
    a2i += 1;
    const p = await ask(wt, frame(0x02, d.id), [0x82, 0xff]);
    evidence.probes["⑧-" + a2i] = p;
    a2.push({ label: "共享 id #" + d.id + "(" + d.path + ") @ " + wt, cls: "两视图同 id（共用 arena 槽）⇒ 必须放行", p, want: 0x82, ok: p.frames.length > 0 && p.frames[0].tag === 0x82 });
  }
}
if (a2RootOnly) {
  a2i += 1;
  const p = await ask(usedMountWt, frame(0x02, a2RootOnly.id), [0x82, 0xff]);
  evidence.probes["⑧-" + a2i] = p;
  a2.push({ label: "根视图独占 id #" + a2RootOnly.id + "(" + a2RootOnly.path + ") @ " + usedMountWt, cls: "另一视图的 id ⇒ 被拒**不算门 bug**（客户端帧混用）", p, want: 0xff, ok: p.frames.length > 0 && p.frames[0].tag === 0xff });
}
step("⑧", "A2 视图归属：共享 id 两视图都放行 · 另一视图独占 id 被拒（不算门 bug）",
  a2.length > 0 && a2.every((x) => x.ok),
  (a2.length
    ? a2.map((x) => x.label + " ⇒ 期望 " + hex(x.want) + " 实得 " + (x.p.frames.length ? hex(x.p.frames[0].tag) : "无帧") + (x.ok ? " ✓" : " ✗") + "（" + x.cls + "）").join(" · ")
    : "**夹具缺失**：两视图 id 无交集且找不到根视图独占件")
  + " · 两视图共享 id 共 " + sharedIds.length + " 个（本项取 " + sharedPick.length + " 个）");

// ── ⑨ A3 带 rule 的 binding 遮蔽格：跳过并标注（当前不可触发，需要 rule；不硬造环境）
skipStep("⑨", "A3 带 rule 的 binding 遮蔽格（文件物理在本 wt storage 内、但被 binding 前缀遮蔽 ⇒ 不在集合里）",
  "**跳过并标注：当前不可触发，需要 rule**。遮蔽只对「有 parent 的 wt（bfork 子 wt）且 bindings 非空」生效"
  + "（store.rs collect_vfs_paths：binding_prefixes 仅在 node.parent.is_some() 时由 bindings 的 my_path 组成）；"
  + "本环境 9 个挂载均无 rule（红队实测），且 bindings/rule 不可从 HTTP/WS 外部观测 ⇒ 活体造不出该格，不硬造。");

// ── ⑩ A4 顺序不变量：新连接上 UP_TREE 必须先于首个 UP_OPEN，且树帧真的登记了 id
// 关键：选一棵**本次从未用 HTTP 取过树**的 wt —— HTTP /fs/tree 自己就会 refresh_tree_json 登记 meta，
// 先取过就判不出「树帧到底有没有登记」。同一连接：先发 0x01（等 0x81）→ 从该帧取 id → 立刻发 0x02。
let p10 = null;
let vWt = null;
let vDoc = null;
{
  const st10 = await status();
  const touched = new Set([GENESIS, usedMountWt, otherOwner].filter(Boolean));
  for (const u of (Array.isArray(st10 && st10.wts) ? st10.wts : []).filter((x) => x && !touched.has(x))) {
    const seq = await askSeq(u, [
      { bytes: frame(0x01, 0), wantTags: [0x81] },
      { bytes: (frames) => { const a = parseTreeFrame(frames.find((f) => f.tag === 0x81)); const h = a && (a.find((e) => String(e.path) === "/CHAT_HIS.md") || a[0]); return h ? frame(0x02, h.id) : null; }, wantTags: [0x82, 0xff] },
    ]);
    const a = parseTreeFrame(seq.frames.find((f) => f.tag === 0x81));
    if (!a || !a.length) continue;                                   // 这棵 wt 没树 ⇒ 顺序不变量无从判，换下一棵
    p10 = seq; vWt = u; vDoc = a.find((e) => String(e.path) === "/CHAT_HIS.md") || a[0];
    break;
  }
}
evidence.probes["⑩"] = p10 || { frames: [], how: "no-virgin-wt", ms: 0 };
const t10 = p10 ? (p10.frames.find((f) => f.tag === 0x81) || null) : null;
const o10 = p10 ? (p10.frames.find((f) => f.tag === 0x82 || f.tag === 0xff) || null) : null;
step("⑩", "A4 顺序不变量：新连接先 UP_TREE 再首个 UP_OPEN（树帧登记的 id 必须可开）"
  + (vWt ? "（wt=" + vWt + " doc=" + (vDoc ? vDoc.path + "#" + vDoc.id : "?") + "）" : ""),
  !!p10 && !!t10 && !!o10 && o10.tag === 0x82,
  (p10
    ? (t10 ? "树帧 0x81 先到（" + (vDoc ? vDoc.path + "#" + vDoc.id : "?") + "）" : "**树帧未到**") + " · 随后 UP_OPEN ⇒ "
      + (o10 ? hex(o10.tag) + (o10.text ? ' " ' + o10.text + ' "' : "") : "无回复") + "（" + p10.how + " " + p10.ms + "ms）"
    : "**夹具缺失**：status.wts 里没有「本次未取过 HTTP 树」且树非空的 wt")
  + " · 期望 0x82 · 注：红时无法从线面区分「树帧没登记 id」与「该 wt 可达性被拒」（两者同为 0xFF）⇒ 与 ⑤/⑧ 同源风险；"
  + "反向「首个 UP_OPEN 早于任何 UP_TREE ⇒ 无 meta ⇒ 0xFF」在活体上**不可构造**（任何取得 id 的途径本身都会登记 meta），"
  + "该方向由 ③ 覆盖，且现行门与修复后同判（非新增）");

// ── ⑪ A5 死 id 过放：跳过并标注（代码级结论，当前无活体样本；不硬造 wt 删除）
const gIds = genesisHidden.map((e) => Number(e.id));
const gMin = Math.min.apply(null, gIds);
const gMax = Math.max.apply(null, gIds);
let gGaps = 0;
for (let i = gMin; i <= gMax; i += 1) if (gIds.indexOf(i) < 0) gGaps += 1;
skipStep("⑪", "A5 死 id 过放（文件已删、arena 槽未清，现状门 ① `meta.wt_uid == conn_wt` 会放行死 id）",
  "**跳过并标注：代码级结论，当前无活体样本**。arena 槽只在 drop_worktree 按 meta.wt_uid 清；实测 "
  + GENESIS + " 视图 id " + gMin + ".." + gMax + " 共 " + gIds.length + " 个、缺口 " + gGaps + " 个 ⇒ 今天不存在"
  + "「有 meta 但文件已死」的 id 可探（删 wt 才会造出来，不硬造）。旧门会拒（不在可达集）/ B4 会拒（按 wt_uid 判死）/ "
  + "空前缀快路径更宽 ⇒ 该格靠代码级评审兜住，本项只标注。");

// ── 汇总
const bad = rows.filter((r) => !r.ok && !r.skip);
const skipped = rows.filter((r) => r.skip);
const graded = rows.length - skipped.length;
evidence.a2SharedIds = sharedIds;
evidence.a4 = { wt: vWt, doc: vDoc };
evidence.rows = rows.map((r) => ({ no: r.no, name: r.name, ok: r.ok, skip: !!r.skip, detail: r.detail }));
for (const p of Object.values(evidence.probes)) {               // 剥掉 raw（帧结构只留 tag/len/peer/doc/条数与载荷预览）
  if (p && Array.isArray(p.frames)) p.frames = p.frames.map((f) => ({ tag: f.tag, name: f.name, len: f.len, peer: f.peer, doc: f.doc, bytes: f.bytes, text: f.text }));
}
try { fs.writeFileSync(EVIDENCE, JSON.stringify(evidence, null, 2)); } catch (e) { note("证据落盘失败：" + String((e && e.message) || e).slice(0, 80)); }
console.log("  原始响应已落盘：" + EVIDENCE);
if (bad.length) {
  console.log("✗ check-admit-gate 红 " + bad.length + "/" + graded + " 项"
    + (skipped.length ? "（另「跳过并标注」" + skipped.length + " 项：" + skipped.map((s) => s.no).join("/") + "，理由见各行）" : "")
    + "：" + bad.map((b) => b.no + " " + b.name).join(" · "));
  process.exit(1);
}
console.log("✓ check-admit-gate 全绿（" + graded + "/" + graded + "）"
  + (skipped.length ? " · 跳过并标注 " + skipped.length + " 项（" + skipped.map((s) => s.no).join("/") + "）" : "")
  + "：genesis 放行 · 他 wt 拒绝 · 不存在拒绝 · 未知 conn_wt 拒绝 · 挂载树放行 · genesis 树帧正常 · boot 声明放行 · A2 视图归属 · A4 顺序不变量");

// 收尾退出：老的测试脚本跑完不收 CDP socket ⇒ 不显式退出就会被自己的 deadline 判成卡死。
// 退出码沿用脚本自己设的 process.exitCode（没设 = 0）；失败路径若已 process.exit(1) 则不受影响。
process.exit(process.exitCode === undefined ? 0 : process.exitCode);
