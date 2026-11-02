// @ts-check
/**
 * client —— 层3 服务：wasm 的对讲机。
 *
 * 一份文件装协议常量表、SAB 双缓冲、worker 装配、会话、帧落地、exec。
 * WS 连接归 wasm（transport 在 wasm 内），本文件一行 WebSocket 不开，只跟 SAB 说话。
 *
 * 纪律（判官 R1/R4）：无层间 import（供应商包 mobx-state-tree 放行）；词汇表受判官约束；
 * 热路径（_dispatchMessage）不带打点。常量表参照 Rust 的 protocol.rs / net_protocol.rs / lpp_protocol.rs。
 *
 * 帧落地契约（实测钉死，见 test/PROTOCOL.md）：
 *   F 帧 → `wt.put(docId, { docId, path, ...交付根 })`（整树换代：店按 docId 调和；**店自己就是 root 节点**）；
 *   P 帧 → assertPatchOps 白名单守卫 + MST 自带 applyPatch（op = RFC6902 与 MST 的交集）；
 *   失败 → 大声报 + 同 peer 重发 UpOpen 全量重同步（禁6）。
 */

// ════════════════════ 协议常量表 ════════════════════

/** 帧头：Tag(1) + Len(4 LE) + Peer(4 LE) + Doc(4 LE) = 13 字节。 */
export const HEADER_SIZE = 13;

/** LPPB 预留头：前 128 字节给读写指针与握手区。 */
const LPPB_RESERVED_SIZE = 128;
const WRITE_PTR_IDX = 0;
const READ_PTR_IDX = 16;
const TAG_SWITCH = 0xFE;
const DEFAULT_CAPACITY = 8 * 1024 * 1024; // 8MB

/** JS <-> wasm（LPPB）消息标签。 */
const Tag = {
  OpenDoc: 0x01,
  CloseDoc: 0x02,
  LocalInsert: 0x03,
  LocalDelete: 0x04,
  UndoReq: 0x05,
  RedoReq: 0x06,
  CursorMove: 0x07,
  ImeInsert: 0x08,
  ImeDelete: 0x09,
  ExplicitUndoRecord: 0x0a,
  ImeBegin: 0x0b,
  ImeCommit: 0x0c,
  ImeCancel: 0x0d,
  ExecReq: 0x0e,   // peer=0/doc=0 系统帧，wasm 透传网关
  ExecRes: 0x0f,
  RemoteOp: 0x10,
  RemoteCursor: 0x11,
  ConnectionState: 0x20,
  RenderDir: 0x81,
  TelemetryProbe: 0x30,
  TelemetryReturn: 0xb0,
  Error: 0xff,
};

/** 坐标坐标掩码与视图掩码（与 wasm 对应）。 */
const MODE_OFFSET = 0x80;
const MODE_MONACO = 0b01;
const MODE_AST = 0b100;

/** RemoteOp 子标签：F 全量 = 3，P 补丁 = 4。 */
const AST_FULL = 3;
const AST_PATCH = 4;

/** 零帧判据期限：区分「慢」与「没有回音」，只报一次，不自动重试。 */
const OPEN_STALL_MS = 4000;

// ════════════════════ SAB 双缓冲（LPPB JS 侧） ════════════════════

class LinearBufferReader {
  constructor(sabList) {
    this.buffers = sabList.map((sab) => this.wrap(sab));
    this.activeIdx = 0;
  }
  wrap(sab) {
    return {
      sab,
      state: new Int32Array(sab, 0, 32),
      data: new Uint8Array(sab),
      capacity: sab.byteLength,
    };
  }
  updateBuffer(idx, newSab) {
    if (idx < 0 || idx > 1) return;
    const oldBuf = this.buffers[idx];
    this.buffers[idx] = this.wrap(newSab);
    if (oldBuf) {
      Atomics.store(oldBuf.state, WRITE_PTR_IDX, -1);
      Atomics.notify(oldBuf.state, WRITE_PTR_IDX, 1);
    }
  }
  async waitForDataAsync(minBytes = 1) {
    const current = this.buffers[this.activeIdx];
    const wPtr = Atomics.load(current.state, WRITE_PTR_IDX);
    const rPtr = Atomics.load(current.state, READ_PTR_IDX);
    if (rPtr + minBytes <= wPtr) return;
    const result = Atomics.waitAsync(current.state, WRITE_PTR_IDX, wPtr);
    if (result.async) await result.value;
  }
  readOne() {
    let loopCount = 0;
    while (loopCount++ < 10) {
      const cur = this.buffers[this.activeIdx];
      const wPtr = Atomics.load(cur.state, WRITE_PTR_IDX);
      const rPtr = Atomics.load(cur.state, READ_PTR_IDX);
      if (rPtr >= wPtr) return null;

      const tag = cur.data[rPtr];
      if (tag === TAG_SWITCH) {
        Atomics.store(cur.state, WRITE_PTR_IDX, LPPB_RESERVED_SIZE);
        Atomics.store(cur.state, READ_PTR_IDX, LPPB_RESERVED_SIZE);
        this.activeIdx = (this.activeIdx + 1) % 2;
        continue;
      }

      if (wPtr - rPtr < HEADER_SIZE) return null;
      const bodyLen =
        cur.data[rPtr + 1] |
        (cur.data[rPtr + 2] << 8) |
        (cur.data[rPtr + 3] << 16) |
        (cur.data[rPtr + 4] << 24);
      const total = HEADER_SIZE + bodyLen;
      if (wPtr - rPtr < total) return null;

      const data = new Uint8Array(total);
      data.set(cur.data.subarray(rPtr, rPtr + total));
      Atomics.store(cur.state, READ_PTR_IDX, rPtr + total);
      return data;
    }
    return null;
  }
}

class LinearBufferWriter {
  constructor(sabList, onUpgrade) {
    this.buffers = sabList.map((sab) => this.wrap(sab));
    this.activeIdx = 0;
    this.onUpgrade = onUpgrade;
    this.resetBuffer(this.buffers[0]);
    this.resetBuffer(this.buffers[1]);
  }
  wrap(sab) {
    return {
      sab,
      state: new Int32Array(sab, 0, 32),
      data: new Uint8Array(sab),
      capacity: sab.byteLength,
    };
  }
  resetBuffer(buf) {
    Atomics.store(buf.state, WRITE_PTR_IDX, LPPB_RESERVED_SIZE);
    Atomics.store(buf.state, READ_PTR_IDX, LPPB_RESERVED_SIZE);
  }
  allocate(size) {
    const needed = size + 1;
    let cur = this.buffers[this.activeIdx];
    let wPtr = Atomics.load(cur.state, WRITE_PTR_IDX);

    if (wPtr + needed > cur.capacity) {
      cur.data[wPtr] = TAG_SWITCH;
      Atomics.store(cur.state, WRITE_PTR_IDX, wPtr + 1);
      Atomics.notify(cur.state, WRITE_PTR_IDX, 1);

      const nextIdx = (this.activeIdx + 1) % 2;
      let nextBuf = this.buffers[nextIdx];
      const nw = Atomics.load(nextBuf.state, WRITE_PTR_IDX);
      const nr = Atomics.load(nextBuf.state, READ_PTR_IDX);
      if (nw !== LPPB_RESERVED_SIZE || nr !== LPPB_RESERVED_SIZE) {
        console.warn("[LPPB] 背压：备用缓冲忙");
        return null;
      }

      const requiredCap = LPPB_RESERVED_SIZE + size + 2048;
      if (requiredCap > nextBuf.capacity) {
        let newSize = requiredCap * 2;
        if (newSize % 4 !== 0) newSize += 4 - (newSize % 4);
        const newSab = new SharedArrayBuffer(newSize);
        nextBuf = this.wrap(newSab);
        this.buffers[nextIdx] = nextBuf;
        this.resetBuffer(nextBuf);
        this.onUpgrade?.(nextIdx, newSab);
      } else if (nextBuf.capacity > DEFAULT_CAPACITY && requiredCap < DEFAULT_CAPACITY) {
        const newSab = new SharedArrayBuffer(DEFAULT_CAPACITY);
        nextBuf = this.wrap(newSab);
        this.buffers[nextIdx] = nextBuf;
        this.resetBuffer(nextBuf);
        this.onUpgrade?.(nextIdx, newSab);
      }

      this.activeIdx = nextIdx;
      cur = nextBuf;
      wPtr = LPPB_RESERVED_SIZE;
    }

    return {
      view: cur.data.subarray(wPtr, wPtr + size),
      offset: wPtr,
      bufferIdx: this.activeIdx,
    };
  }
  commit(allocCtx, bytesWritten) {
    const { bufferIdx, offset } = allocCtx;
    const cur = this.buffers[bufferIdx];
    const newPtr = offset + bytesWritten;
    Atomics.store(cur.state, WRITE_PTR_IDX, newPtr);
    Atomics.notify(cur.state, WRITE_PTR_IDX, Infinity);
  }
}

// ════════════════════ 零帧判据（一次性） ════════════════════

function createOpenWatch(ms, onStall) {
  let state = "waiting";
  let timer = setTimeout(function () {
    if (state !== "waiting") return;
    state = "stalled";
    onStall({ ms });
  }, ms);
  return {
    note() { if (state === "waiting") { state = "answered"; clearTimeout(timer); } },
    dispose() { if (state === "waiting") { state = "disposed"; clearTimeout(timer); } },
    get state() { return state; },
  };
}

// ════════════════════ 失败协议（exec 结果的归一） ════════════════════

function isMdioError(text) {
  const t = String(text == null ? "" : text).trim();
  if (!t) return false;
  if (/^\s*(?:-[ \t]*)?mdio_error\b/.test(t)) return true;
  return t.indexOf("\n") < 0 && /^(?:File not found|No such file or directory)\b/.test(t);
}

// ════════════════════ 对讲机 ════════════════════

import { types, applyPatch, getRoot, getParent, hasParent, isAlive, getSnapshot } from "mobx-state-tree";
import { observable } from "mobx";

// 动词白名单守卫：MST 对未知动词静默忽略，禁6 要求大声报（可测导出）。
export function assertPatchOps(ops) {
  for (const op of ops) {
    if (!op || typeof op !== "object" || typeof op.op !== "string"
      || ["add", "remove", "replace", "replace_attr"].indexOf(op.op) < 0) {
      throw new Error("未知补丁动词：" + JSON.stringify(op).slice(0, 100));
    }
  }
}

// 交付树容器（MST）：模型键 = 当前交付键的镜像（nt/lv/t/ct…）；引擎交付键变了就同步增键。
// P 补丁用 MST 自带的 applyPatch——op 语义 = RFC6902 与 MST 的交集（实测一致），不自造。
// 引擎给什么存什么：字段形状必须宽容（实测 /demos/antd/README.md 的 h.t 是 inline 节点数组、tb.cols 是 string，
// 旧模型 t:string 会让整棵树转换失败 ⇒ 快照不落地、UI 静默卡「等待首帧」）。
export const Node = types.model("Node", {
  nt: types.optional(types.string, ""),
  lv: types.optional(types.number, 0),
  // t = 标题：字符串（多数）或 inline 节点数组（富标题，实测）
  t: types.optional(types.union(types.string, types.frozen()), ""),
  ct: types.optional(types.union(types.string, types.array(types.late(() => Node)), types.frozen()), ""),
  // cd = 代码内容键（引擎契约：code span/fence 的内容在 cd，不在 ct——effect.rs「JSON key 用 cd 而非 ct」）。
  cd: types.optional(types.union(types.string, types.frozen()), ""),
  // h = 链接/自动链接地址（全库键普查：仅 lk/al 带，117 处）。没声明 ⇒ 建模时被静默丢弃、链接丢 href。
  h: types.optional(types.string, ""),
  // s/alt/ti = 图片的 src/alt/title（core 的 IMAGE_ATTRS = { s, alt, ti }，effect.rs:1932-1936）。
  // **投影名 `al` 不存在**（业主口径「旧的必须删 · 没有兜底」）：它只是给投影留的第二个名字，
  // 声明它就是承认两套名字。没有它时不声明即被 MST 丢弃 —— 那正是"看得见"的错法。
  s: types.optional(types.union(types.string, types.frozen()), ""),
  alt: types.optional(types.union(types.string, types.frozen()), ""),
  ti: types.optional(types.union(types.string, types.frozen()), ""),
  // 表格列定义等引擎附加字段：形状不定（string | array），一律 frozen 兜底
  // 表格列定义：引擎给 string（实测 `/demos/antd/README.md` 的 `tb.cols`），也可能给数组 ⇒ 联合类型（不再是裸 frozen）。
  cols: types.optional(types.union(types.string, types.frozen()), ""),
  // **`mark` 恢复声明**（业主口径修正）：它是**交付键**（`block.rs:4072` 的 BlockProj 枚举里有 `mark`），
  //   按「零读取就删」删掉是错的：**结构即数据 ⇒ 交付键一律声明**（有没有读者是另一回事）。
  mark: types.optional(types.frozen(), undefined),
  // `match` = 引擎的**归属注入**（`projection.rs:3281`「归属注入版（N5，F 帧）：每块级节点对象带 match
  //   （无认领 ⇒ 逐字节不变）」；`store.rs:745` 在 needs_fanout 时注入）。同样按交付键声明。
  match: types.optional(types.frozen(), undefined),
}).views((self) => ({
  /** 孩子（**普通数组**）：MST 可观察数组不是真数组 ⇒ 一处收口（DocPane / std 都读它）。 */
  get kids() { const c = self.ct; if (!c || typeof c === "string" || typeof c.length !== "number") return []; return Array.from(c); },
  /** 祖先链（含本站与文档根）：**无参 getter = computed**（官方：带参 view 不缓存；观察时按节点缓存）。
   *  · 两跳取「模型父」：`getParent` 一跳给的是**数组包装**，两跳才是模型父（实测 5 种树形状逐节点 0 不一致）；
   *  · **根没有父是正常终止**（用 `hasParent` 判，不靠 catch）—— 曾把 try/catch 整段删掉，真页面立刻报错、文档面整块错误卡；
   *  · 边界：父不是**节点形状**（没有 `nt`，即 MD 店/持有者）就停 —— 链不许走出文档树；
   *  · 顺序 = **根在前、本站在尾**（与 IR 从根往下匹配一致；写反会全不命中）。 */
  get chain() {
    const out = [];
    let cur = self;
    while (cur && isAlive(cur)) {
      out.push(cur);
      if (!hasParent(cur)) break;
      const up1 = getParent(cur);
      if (!up1) break;
      const up2 = getParent(up1);
      if (!up2) break;
      if (typeof up2.nt !== "string") break;
      cur = up2;
    }
    // **根在前、本站在尾** —— 这是 `ancestorsOf` 的既有契约（判据 `check_ancestors_root`「链首是根」、`check_md_store`「chain[0] === 店」都按它写）；
    // IR 的匹配也是从根往下走（app 注入的 `chainWalk`），顺序反了会全不命中（本轮实测踩过）。
    return out.reverse();
  },
  /** 命中与可见：**一次判定、两个名字**（`gates` 兜住那条链与那次表扫，`claim`/`visible` 只是读它的结果）。
   *  声明店从**主树根上的 `ui` 子树**取（同树 getRoot；services 层不许层间 import，判官 R1）；
   *  脱离店（判据里裸建 `Node.create`）⇒ 无认领、可见 —— 与旧行为的空表语义一致。 */
  get gates() {
    let store = null;
    try { store = getRoot(self).decl; } catch (e) { store = null; }
    if (!store || typeof store.gates !== "function") return { claim: null, visible: true };
    return store.gates(self);
  },
  get claim() { return self.gates.claim; },
  get visible() { return self.gates.visible; },
}));
/**
 * **MD —— 一篇文档的店，而它自己就是这棵树的 root 节点**（波 4）。
 *   · 为什么这样：业主口径「root node 本质 node，但可以自己成 store」—— 把结构（`nt` / `ct`，交付 JSON 逐键镜像）
 *     和这一层的身份（`docId` / `path`）放在**同一个店**上 ⇒ **没有多包一层** ⇒ 渲染面看到的 `ui.doc` 就是它，
 *     祖先链爬到它为止（它之上只有持有者，不是节点），不会出现「爬到未挂载的父级」那类事故。
 *   · 键 = `docId`（交付身份，`Wt.by` 的键）：`types.map` 按**键**调和 ⇒ 同一档的 F/P 帧复用同一家店。
 *   · `path` 是**字段**（可改）：改名不该换店。
 *   · **交付信封的键逐字声明**（业主口径：Rust 明文有的键，模型对应上，不许"静默丢"）——
 *     出处 `mdio_core/src/projection.rs:3270-3275`：root 信封 = `nt` + **拓扑取值键** + **生成时刻** + 可选**文档名** + `ct`（取值与键名见该处明文）。
 *     判官 R6c 同步放宽为「**禁逻辑用词，不禁声明**」：字段声明与快照键字面量放行，拿它做判断的写法仍红。
 */
// **店 = 节点 + 身份字段**：`types.compose` 把 `Node` 的**结构字段与派生视图**（`kids`/`chain`/`gates`/`claim`/`visible`）一并带过来 ——
// 这样「root 也是 node」是**字面为真**（不是两套模型长得像），也不需要在两处重复声明同一个视图（一处所有者）。
export const MD = types.compose("MD", Node, types.model({
  docId: types.number,
  path: types.optional(types.string, ""),
  // ── 交付信封三键（原先未声明 ⇒ 被 MST 静默丢弃）
  mode: types.optional(types.string, ""),
  timestamp: types.optional(types.number, 0),
  md: types.optional(types.string, ""),
})).actions((self) => ({
  setPath(p) { self.path = String(p == null ? "" : p); },
}));
/** 文件面条目：通道给的 `{ path, docId }`（一个 wt 作用域内的挂载树）。
 *  `id` 是**视图**（通道/UI 习惯用 `id`）—— 不存第二份字段：**MST 的 view 就是干这个的**。 */
export const TreeEntry = types.model("TreeEntry", {
  path: types.string,
  docId: types.number,
}).views((self) => ({
  get id() { return self.docId; },
}));

/**
 * **Wt —— 工作树一层店**（业主口径：wt 套一个 md map，DevView 直接 computed 很方便）。
 *   · `uid`：这一层的标识（值由**装配点**按 `?wt=<uid>` 给；框架不认识具体 wt）；
 *   · `by`：`docId(字符串键) -> MD`（map 按**键**调和 ⇒ 同档 F/P 帧复用同一条店）；
 *   · `entries`：**文件面**（通道 `onTree` 来的 `{path,docId}`）；DevView 的文件视图将来直接 computed 它（专项）；
 *   · 写操作一律走 action（MST 的写只能在 action 里 —— R8.2 实测）。
 */
export const Wt = types.model("Wt", {
  uid: types.optional(types.string, ""),   // 调用方给的 wt 标识（框架**不认识任何具体 wt**；值从装配点来）
  by: types.map(MD),
  entries: types.array(TreeEntry),
}).volatile((self) => ({
  /** 三态盒（阶段①-3）：`{ [docId(string)]: 'pending'|'ready'|'error' }`（普通对象；volatile + box，不进快照）。 */
  phasesBox: observable.box({}, { deep: false }),
  /** 装载器持有位：`Wt` 不 import client（服务层纪律）⇒ 装配点把 client 填进来。 */
  loaderBox: observable.box(null, { deep: false }),
})).actions((self) => ({
  put(docId, snap) { self.by.set(String(docId), snap); },
  drop(docId) { self.by.delete(String(docId)); },
  clearAll() { self.by.clear(); },
  putEntries(list) { self.entries.replace(Array.isArray(list) ? list : []); },
  setLoader(c) { self.loaderBox.set(c || null); },
  /** **需求入口**：observe(ref) = 「我要这篇」（幂等；不计数、不做差集、不碰释放规则）。
   *  ref 以「/」开头 ⇒ `byPath` 解析 docId；数字 ⇒ 当 docId；命中 `by.get` ⇒ 立即 `ready`；否则 `pending` + `loader.ensure`。 */
  observe(ref) {
    const path = typeof ref === "string" && ref.charAt(0) === "/" ? ref : null;
    const docId = typeof ref === "number" ? ref : (path ? self.byPath[path] : undefined);
    if (typeof docId !== "number") return { ok: false, reason: "unknown-path" };
    if (self.by.get(String(docId))) { self.note(docId, "ready"); return { ok: true, docId, already: true }; }
    self.note(docId, "pending");
    const L = self.loaderBox.get();
    if (L && typeof L.ensure === "function") L.ensure(docId);
    return { ok: true, docId, already: false };
  },
  /** 三态写点（只由既有钩子调）：整体置换盒内对象，同值不通知；error 时打一行 reason。 */
  note(docId, phase, reason) {
    const key = String(docId);
    const cur = self.phasesBox.get() || {};
    if (cur[key] === phase) return;
    const next = Object.assign({}, cur);
    next[key] = String(phase);
    self.phasesBox.set(next);
    if (reason) console.warn("[client] 三态 " + key + " = " + phase + " · " + reason);
  },   // 声明店晚到（宿主先建 client）⇒ 装配点建好后再填
})).views((self) => ({
  /** 文件视图：**无参 getter = MST computed**（带参 view 不缓存 —— §4 表 / R1.14）。
   *  返回**普通数组**（不是 MST 数组）：DevView 那边 `Array.isArray(entries)` 才成立，且这里就是唯一的派生点，
   *  宿主不许再手搓一份 `map`。 */
  get files() { return self.entries.map((e) => ({ path: e.path, id: e.docId })); },
  /** 路径索引视图：**`path -> docId`**（路由参数是路径，而 `by` 的键是 docId —— `docs/isolation/reverse-dep/04-verdict.md:24`、`docs/isolation/appview-模型-路由与数据.md:64`）。
   *  数据源与 `files` 同一个 `entries`（`:341-346`）；**无参 getter = MST computed** ⇒ `entries` 换代（`putEntries` `:369`）即自动重算。
   *  返回**普通对象**（不是 MST 节点、不是 MST map）：app 侧 `wt.byPath[path]` 零 MST API；键是通道给的原始路径（未做归一）。 */
  get byPath() { const out = {}; for (const e of self.entries) out[e.path] = e.docId; return out; },
  /** 三态读面：`'pending' | 'ready' | 'error' | undefined`。 */
  phaseOf(docId) { const m = self.phasesBox.get() || {}; return m[String(docId)]; },
}));

/**
 * **Wts —— 数据顶层（node → md → wt → wts 的 wts）**（app 时代一棵主树的 wts 子树）。
 *   · `wts`：uid -> Wt 的树册（访问过的每棵树都在册 ⇒ 换 wt 复用、切回不白屏）；
 *   · `currentWT`：当前指针 —— **换 wt 只改这一个字段**，所有读者经 `wt` 视图自动跟随；
 *   · `get wt()`：当前树（无参 getter = computed）。
 * 装配方式：runtime（demo）把它拼进总 store（UiStore 的 wts 子树）；client 经 `o.wts` 拿到后
 *   `ensure(uid)` + `wts.get(uid)` 当自己的 wtStore（缺容器时退单棵 Wt.create，旧行为不变）。
 */
export const Wts = types.model("Wts", {
  wts: types.map(Wt),
  currentWT: types.optional(types.string, ""),
}).views((self) => ({
  get wt() { return self.wts.get(self.currentWT) || null; },
})).actions((self) => ({
  setWT(uid) { self.currentWT = String(uid || ""); },
  ensure(uid, snap) { if (!self.wts.has(uid)) self.wts.set(uid, snap || { uid, by: {}, entries: [] }); },   // set(key,value)；put 是单参（按 identifier）
}));

const NOOP_PERFMARK = () => {};

export class MdioClient {
  constructor(opts) {
    const o = opts || {};
    // 适配器注入（wasm web mod）：装配点把 ui_wasm 的 createWorker 递进来；缺席 ⇒ init 时大声报错（裸路径不存在，无回退）。
    this._createWorker = typeof o.createWorker === "function" ? o.createWorker : null;
    this.worker = null;
    this.writer = null;
    this.reader = null;
    this.isReady = false;
    this._readLoopRunning = false;
    this.sessions = new Map();      // docId -> Set<session>
    // 工作树店：文档店集合（键 = docId）+ 文件面（entries）—— F/P 帧与 onTree 落这里（波 4）
    // 写操作一律走 Wt 的 action（帧处理函数在 action 作用域外）
    // 工作树店来源：有容器（总 store 的 wts 子树）则 ensure 后取那棵，切回同 uid 复用同店；
    // 无容器（旧装配/独立运行）退单棵 Wt.create —— 兼容旧行为。
    const uid0 = String(o.uid || "");
    if (o.wts && typeof o.wts.ensure === "function") {
      o.wts.ensure(uid0);
      this.wtStore = o.wts.wts.get(uid0);
    } else {
      this.wtStore = Wt.create({ uid: uid0, by: {}, entries: [] }, o.env || {});
    }   // uid 由调用方给（mdio_ui 里不出现任何 wt 名）；env = 共享装配总线（S2）
    this._rev = new Map();          // docId -> 帧代数
    this.pendingExec = new Map();   // exec_id -> {resolve, reject}
    this._execSeq = 0;
    this._probeActive = false;
    this._openWatches = new Map();  // docId -> watch（一次性）
    this._openStalls = 0;

    this._lastOpenStall = null;
    this._orphanFrames = 0;
    this._connState = "";
    this._listeners = new Set();    // 帧 bus
    this.onConnectionState = o.onConnectionState || null;
    this.onTree = o.onTree || null;
    this.onDocGone = o.onDocGone || null;
    this.onOpenStall = o.onOpenStall || null;
    this._tel = null;
    this._telemetryRaw = "";   // 最近一次 WASM 分层打点（requestTelemetry → telemetry_data）
    this._perfMark = NOOP_PERFMARK;
  }

  // ── telemetry 注入（只在边界消费；不开时零开销） ──
  setTelemetry(tel) { this._tel = tel; }
  setPerfMark(fn) { this._perfMark = typeof fn === "function" ? fn : NOOP_PERFMARK; }

  /** exec：经 wasm 通路（LPPB 0x0E → wasm 透传 → 网关）。失败抛错（失败协议在此归一）。 */
  exec(code, { active_doc = "", wt_uid = null } = {}) {   // 步骤 3 补（复核 F4）：默认**不带** wt_uid
    if (!this.writer) return Promise.reject(new Error("client 未初始化"));
    const exec_id = ++this._execSeq;
    // 步骤 3：连接自带 wt ⇒ 载荷默认**不带** wt_uid（服务端用连接上下文）；
    // 只有显式传了 wt_uid（HTTP/宿主场景）才带上，作为显式覆盖。
    const body = { exec_id, code, active_doc };
    if (wt_uid) body.wt_uid = wt_uid;
    const payload = new TextEncoder().encode(JSON.stringify(body));
    const t0 = performance.now();
    this._perfMark("exec_sent");
    return new Promise((resolve, reject) => {
      this.pendingExec.set(exec_id, { resolve, reject });
      this.sendPayload(0, 0, Tag.ExecReq, payload);
    }).then((raw) => {
      this._tel?.sample("exec_roundtrip", performance.now() - t0);
      this._perfMark("exec_done");
      if (isMdioError(raw)) {
        const line = String(raw).trim().split("\n").find((l) => l.trim()) || "";
        throw new Error(line.slice(0, 160));
      }
      return raw;
    });
  }

  /** 取一次 WASM 侧分层打点：worker 回 `telemetry_data` ⇒ 落在 `_telemetryRaw` 与页面 console。 */
  requestTelemetry() {
    // 观测口：这次请求的墙钟（与 telemetry_data 的 recvWall 配对 ⇒ 纯往返地板，不含 wasm 计算）
    try { console.log("[TELREQ] wall=" + Date.now()); } catch (e) { /* ignore */ }
    try { this.worker?.postMessage({ type: "get_telemetry" }); } catch (e) { /* 未初始化时静默 */ }
  }

  /** 遥测探针往返：发 0x30 等 0xB0，返回 RTT ms（超时/未初始化 → null）。 */
  probe(timeoutMs = 2000) {
    if (!this.writer || this._probeActive) return Promise.resolve(null);
    this._probeActive = true;
    const t0 = performance.now();
    const prevRecv = this.onAfterRecv;
    return new Promise((resolve) => {
      const finish = (v) => {
        clearTimeout(timer);
        this.onAfterRecv = prevRecv;
        this._probeActive = false;
        resolve(v);
      };
      const timer = setTimeout(() => finish(null), timeoutMs);
      this.onAfterRecv = (docId, tag, data) => {
        if (tag === Tag.TelemetryReturn) { finish(performance.now() - t0); return true; }
        if (prevRecv) return prevRecv(docId, tag, data);
        return false;
      };
      this.sendPayload(0, 0, Tag.TelemetryProbe, new Uint8Array(0));
    });
  }

  // ── wasm worker 装配 ──
  async init() {
    const upSab0 = new SharedArrayBuffer(DEFAULT_CAPACITY);
    const upSab1 = new SharedArrayBuffer(DEFAULT_CAPACITY);
    const downSab0 = new SharedArrayBuffer(DEFAULT_CAPACITY);
    const downSab1 = new SharedArrayBuffer(DEFAULT_CAPACITY);

    this.writer = new LinearBufferWriter([upSab0, upSab1], (idx, newSab) => {
      this.worker?.postMessage({ type: "update_upstream_buffer", data: { idx, sab: newSab } });
    });
    this.reader = new LinearBufferReader([downSab0, downSab1]);

    // wasm 适配器注入（无裸路径回退）：装配点必须给 createWorker，缺席 = 大声报错（不静默）。
    if (!this._createWorker) throw new Error("wasm 适配器未注入：装配点应把 ui_wasm/worker 的 createWorker 递进来");
    this.worker = this._createWorker();
    this.worker.onerror = (e) => console.error("[client] worker error:", e);
    this.worker.onmessageerror = (e) => console.error("[client] worker message error:", e);
    this.worker.onmessage = (e) => this._onWorkerMessage(e);

    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => reject(new Error("worker 初始化超时")), 15000);
      const handler = (msg) => {
        if (msg.data.type === "init_success") {
          clearTimeout(timeout);
          this.worker.removeEventListener("message", handler);
          this.isReady = true;
          this._startReadLoop();
          resolve(msg.data.data);
        } else if (msg.data.type === "error") {
          clearTimeout(timeout);
          this.worker.removeEventListener("message", handler);
          reject(new Error(msg.data.error));
        }
      };
      this.worker.addEventListener("message", handler);
      this.worker.postMessage({
        type: "init",
        data: { peerId: 0, upSab0, upSab1, downSab0, downSab1 },   // wasmUrl 由 worker 静态 import 胶水注入
      });
    });
  }

  /**
   * 步骤 4（身份单层）：本连接的**唯一** peer 身份。视图只是 sink。
   *
   * 与之前那次失败尝试的区别（那次 重挂 0/3）：现在引擎**每篇文档只发一帧**（DnAst 去重 ✓）、
   * 且客户端按**子标签家族**路由（不再靠 peer 选会话 ✓）；OpenDoc 每次都发 UpOpen（lib.rs:1295 ✓）
   * ⇒ 新 sink 加入已开文档时内容照常下发。**身份只有这一层**（视图是 sink，不再各自持 id）。
   */
  get peerId() {
    if (!this._peerId) this._peerId = (Math.random() * 0xffffffff) >>> 0;
    return this._peerId;
  }

  async connect(url) {
    if (!this.worker) throw new Error("worker 未初始化");
    if (!url) throw new Error("connect: 缺少网关地址（同源相对，由装配点注入）");
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => reject(new Error("连接超时")), 10000);
      const handler = (msg) => {
        if (msg.data.type === "connected") {
          clearTimeout(timeout);
          this.worker.removeEventListener("message", handler);
          resolve(msg.data.data);
        } else if (msg.data.type === "error") {
          clearTimeout(timeout);
          this.worker.removeEventListener("message", handler);
          reject(new Error(msg.data.error));
        }
      };
      this.worker.addEventListener("message", handler);
      this.worker.postMessage({ type: "connect", data: { url } });
    });
  }

  _onWorkerMessage(e) {
    const { type, data } = e.data;
    if (type === "update_downstream_buffer") {
      this.reader?.updateBuffer(data.idx, data.sab);
    } else if (type === "telemetry_data") {
      // WASM 侧分层打点（WASM_local_proc / WASM_net_proc）⇒ 主线程。这条消息**没有接收方**
      // （worker 的 get_telemetry 分支无人调用）⇒ 数据一直没人收；这里收下并打给页面 console。
      // 注：**不在这里 fetch 网关**（判据 R6b「数据经 client」禁止核心件绕过 client 直连 HTTP）；
      // 要接网关 /telemetry/js-flush 由装配点（demo 壳）做。
      this._telemetryRaw = String(data || "");
      try { console.log("[WASM_TEL] recvWall=" + Date.now() + " " + this._telemetryRaw.slice(0, 500)); } catch (e) { /* ignore */ }
    } else if (type === "console") {
      const fn = data?.level === "error" ? console.error : data?.level === "warn" ? console.warn : console.log;
      fn("[worker:" + (data?.level || "?") + "] " + (data?.text || ""));
    } else if (type === "error") {
      // worker 里的致命错误（含 run_loop 停机）。这条消息**没有接收方** ⇒ 引擎死了没人知道。
      this._engineError = e.data.error || "unknown";
      console.error("[client] worker 错误：引擎可能已停机 ⇒", this._engineError);
      try { this.onWorkerError?.(this._engineError); } catch (err) { /* 宿主回调不许影响接收 */ }
    }
  }

  _startReadLoop() {
    if (this._readLoopRunning) return;
    this._readLoopRunning = true;
    const process = async () => {
      try {
        while (this._readLoopRunning) {
          await this.reader.waitForDataAsync(1);
          const msg = this.reader.readOne();
          if (msg) this._dispatchMessage(msg);
        }
      } catch (err) {
        console.error("[client] read loop:", err);
        await new Promise((r) => setTimeout(r, 50));
      }
      if (this._readLoopRunning) setTimeout(() => process(), 0);
    };
    process();
  }

  /** 热路径：逐消息分发。此函数体不带打点。 */
  _dispatchMessage(data) {
    if (data.length < HEADER_SIZE) return;

    const dv = new DataView(data.buffer, data.byteOffset, 13);
    const tag = dv.getUint8(0);
    const peerId = dv.getUint32(5, true);
    const docId = dv.getUint32(9, true);

    if (this.onAfterRecv?.(docId, tag, data)) return;

    if (tag === Tag.ConnectionState) {
      const states = ["connected", "disconnected", "reconnecting", "syncing", "idle"];
      this._connState = states[data[HEADER_SIZE]] || "";
      this.onConnectionState?.(this._connState);
      return;
    }

    // 文件树（docId=0，wasm 把 DnTree 包成 RemoteOp 或 RenderDir）
    if (docId === 0 && (tag === Tag.RemoteOp || tag === Tag.RenderDir)) {
      const payload = data.subarray(HEADER_SIZE);
      try {
        const entries = JSON.parse(new TextDecoder().decode(payload));
        this.onTree?.(entries);
        backfillPaths(this, entries);   // 文件面晚于帧到达 ⇒ 给已建店但 path 为空的文档店补身份
      } catch (e) {
        console.error("[client] 文件树帧解析失败:", e);
      }
      return;
    }

    // exec 响应（系统帧），exec_id 关联请求
    if (tag === Tag.ExecRes) {
      try {
        const { exec_id, raw } = JSON.parse(new TextDecoder().decode(data.subarray(HEADER_SIZE)));
        const p = this.pendingExec.get(exec_id);
        if (p) {
          this.pendingExec.delete(exec_id);
          p.resolve(raw ?? "");
        } else {
          console.warn("[client] ExecRes 未知 exec_id:", exec_id);
        }
      } catch (e) {
        console.error("[client] ExecRes 解析失败:", e);
      }
      return;
    }

    // 文档已不存在（服务端 DN_ERR）：本地会话收掉，宿主必须可见
    if (tag === Tag.Error) {
      let reason = "";
      try { reason = new TextDecoder().decode(data.subarray(HEADER_SIZE)); } catch (e) { /* ignore */ }
      this.wtStore.note(docId, "error", reason || "DN_ERR");
        console.error("[client] 文档不存在（服务端回 " + (reason || "DN_ERR") + "）doc=" + docId);
      const gone = this.sessions.get(docId);
      if (gone) {
        for (const s of [...gone]) { try { s.dispose(); } catch (e) { /* ignore */ } }
        this.sessions.delete(docId);
      }
      this.wtStore.drop(docId);
      try { if (typeof this.onDocGone === "function") this.onDocGone(docId, reason); } catch (e) { /* 宿主回调不许影响会话 */ }
      return;
    }

    const sessions = this.sessions.get(docId);
    if (sessions) {
      const watch = this._openWatches.get(docId);
      if (watch) { watch.note(); this._openWatches.delete(docId); }
      const st = data.length > 13 ? data[13] : 0;
      // F/P 帧由 wasm 逐 peer 各下行一份：按帧内 peer 只投对应会话；
      // peerId===0 是系统/全量帧，放行给全部会话。
      const astFrame = tag === Tag.RemoteOp && (st === AST_FULL || st === AST_PATCH);
      let delivered = 0;
      for (const s of sessions) {
        // 步骤 4：**帧头 peer 不承担选会话的职责**。引擎对每篇文档只发一帧（见 lib.rs DnAst），
        // 所以 AST 帧按**家族**投给所有要 AST 的 sink；Monaco 的 sink 不消费 AST 子标签、
        // AST 视图不消费 CRDT 子标签 ⇒ 不会双应用。下面这条例外是给 **AST 模式会话**的（peer 不匹配也放行）。
        if (astFrame && peerId !== 0 && s._peerId !== peerId && !(s._mask & MODE_AST)) continue;
        s.handleMessage(tag, data);
        delivered += 1;
      }
      // 帧无人接收必须响：订阅用了过期 docId / peer 不匹配 / 会话已关——静默就再也没帧了。
      // 口径 = **所有**帧（只统计 AST 帧 ⇒ CRDT 帧投给 0 个会话时无声）。
      if (delivered === 0) {
        this._orphanFrames += 1;
        if (this._orphanFrames <= 5 || this._orphanFrames % 50 === 0) {
          console.warn("[client] 帧无人接收 #" + this._orphanFrames,
            { docId, tag: "0x" + tag.toString(16), st, peerId, astFrame,
              peers: Array.from(sessions).map((s) => s._peerId) });
        }
      }
    }
  }

  // ── 会话 ──
  _armOpenWatch(docId) {
    if (this._openWatches.has(docId)) return;
    this._openWatches.set(docId, createOpenWatch(OPEN_STALL_MS, (info) => {
      this._openWatches.delete(docId);
      this._openStalls += 1;
      this._lastOpenStall = { docId, ms: info.ms, at: Date.now() };
      this.wtStore.note(info.docId, "error", "zero-frame " + info.ms + "ms");
        console.error("[client] 打开文档后 " + info.ms + "ms 内没有任何帧（零帧）——网关侧会话/AST 桥可能已停", this._lastOpenStall);
      try { if (this.onOpenStall) this.onOpenStall(docId, this._lastOpenStall); } catch (e) { /* 宿主回调不许影响会话 */ }
    }));
  }

  _sendOpenDoc(peerId, docId, modeTag, modeMask = MODE_MONACO) {
    const payload = new Uint8Array(1);
    payload[0] = modeMask;
    this.sendPayload(peerId, docId, modeTag, payload);
  }

  _sendHeaderOnly(tag, peerId, docId) {
    if (!this.writer) return;
    const alloc = this.writer.allocate(13);
    if (!alloc) return;
    const dv = new DataView(alloc.view.buffer, alloc.view.byteOffset, 13);
    dv.setUint8(0, tag);
    dv.setUint32(1, 0, true);
    dv.setUint32(5, peerId, true);
    dv.setUint32(9, docId, true);
    this.writer.commit(alloc, 13);
  }

  sendPayload(peerId, docId, tag, payload) {
    if (this.onBeforeSend?.(docId, tag, payload)) return;
    if (!this.writer) return;
    // 【incr 起点 · 信息进入 wasm】写入上环**之前**打点。Monaco 编辑 / exec / 模式帧等
    // **所有**可能引起 PJ 的触发都经此 ⇒ incr = 此刻 → 下一个 PJ 出 wasm（出口见 createAstView 的 notify）。
    this._wasmInAt = (typeof performance !== "undefined" && performance.now) ? performance.now() : Date.now();
    this._wasmInWall = Date.now();   // 墙钟锚点（与引擎自报的 WASM_*_ts 同基准，可对齐）
    const totalSize = 13 + payload.length;
    const alloc = this.writer.allocate(totalSize);
    if (!alloc) { console.warn("[client] allocate 失败（size=" + totalSize + "）"); return; }
    const dv = new DataView(alloc.view.buffer, alloc.view.byteOffset, 13);
    dv.setUint8(0, tag);
    dv.setUint32(1, payload.length, true);
    dv.setUint32(5, peerId, true);
    dv.setUint32(9, docId, true);
    alloc.view.set(payload, 13);
    this.writer.commit(alloc, totalSize);
  }

  _addSession(peerId, docId, session, modeTag, modeMask = MODE_MONACO) {
    if (!this.sessions.has(docId)) this.sessions.set(docId, new Set());
    session._mask = modeMask;   // 步骤 4：路由按**家族**判（引擎对每篇文档只发一帧）
    this.sessions.get(docId).add(session);
    this._sendOpenDoc(peerId, docId, modeTag, modeMask);
    this._armOpenWatch(docId);
  }

  /** 通用适配器注册（编辑器等第 5 层用；session 实现 handleMessage + dispose）。 */
  registerProvider(peerId, docId, session, modeTag, modeMask = MODE_MONACO) {
    session._peerId = peerId;
    this._addSession(peerId, docId, session, modeTag, modeMask);
  }

  /** 打开文档（渲染树视图）：F/P 帧落到 trees[docId]，经帧 bus 通知。 */
  open(docId, opts) {
    const o = opts || {};
    // 幂等判据 = **已经有 AST 视图**（不是“有任何 session”）：
    //   Monaco app 可能先为同一篇注册了 MODE_MONACO session（devview 左槽先开档是常态），
    //   若按“任何 session 非空”就早退 ⇒ AST 视图不注册 ⇒ 中心渲染面 model=null ⇒ 空。
    //   实测：Monaco 先开 /addr_p7.md，再在中心点同一篇 ⇒ 中心空白；反过来正常。
    const cur = this.sessions.get(docId);
    if (cur) { for (const s of cur) { if (s && (s._mask & MODE_AST)) return true; } }
    const view = createAstView(this, docId, o.silent);
    this.registerProvider(view._peerId, docId, view, Tag.OpenDoc, MODE_AST);
    return true;
  }

  /** 用户按一下「重开」：对全部会话重发一次 UP_OPEN 并重新武装零帧判据。 */
  reopen(docId) {
    const sessions = this.sessions.get(docId);
    if (!sessions || sessions.size === 0) return false;
    for (const s of sessions) {
      try { this._sendOpenDoc(s._peerId || 0, docId, Tag.OpenDoc, MODE_AST); } catch (e) { /* 连接已断时忽略 */ }
    }
    this._armOpenWatch(docId);
    return true;
  }

  /**
   * 关闭**一个视图**的会话（视图粒度 —— 与引擎 close_view 语义对齐）。
   * 只摘自己这一个 session、只发自己 peerId 的 CloseDoc；订阅是否真退，由 wasm 按
   * 「本连接还有没有别的视图要这篇」判断（lib.rs:1135-1159 的 still_wanted）。
   * 用整篇粒度的 close(docId) 会连带退掉同 doc 的 AST 视图 ⇒ 别在卸载单个面板时用它。
   */
  closeSession(docId, session) {
    const sessions = this.sessions.get(docId);
    if (!sessions || !sessions.has(session)) { try { session.dispose(); } catch (e) { /* ignore */ } return false; }
    sessions.delete(session);
    try { session.dispose(); } catch (e) { /* ignore */ }
    // 切档 ≠ 退订（方案 §5.2② ✗）：视图消失**不再发 CloseDoc**。
    // 文档在引擎里保持订阅与会话；再次打开走既有的「已开文档」路径补齐内容
    //   （lib.rs OpenDoc: already ⇒ push_full_text ✓；AST 视图常驻 ⇒ 树持续更新 ✓）。
    // 真退订只由 releaseAll(docId)（域级）或连接结束（切 wt ⇒ 网关 detach_peer）发生。
    if (sessions.size === 0) {
      this.sessions.delete(docId);
      const w = this._openWatches.get(docId);
      if (w) { w.dispose(); this._openWatches.delete(docId); }
    }
    return true;
  }

  /**
   * **窄接口**（方案 §7 步骤 2）：ensure = 幂等"我要这篇"（复用 OpenDoc 语义 ✓）。
   * 视图与声明层只该用它：不会退订、也不动别人的视图。
   */
  ensure(docId, opts) {
    if (!this.wtStore.by.get(String(docId))) this.wtStore.note(docId, "pending");   // 发起装载 ⇒ pending
    return this.open(docId, opts);
  }

  /**
   * **窄接口**：releaseAll = 唯一真退订入口（整篇粒度，逐视图发 CloseDoc ⇒ ⇒ ⇒ ⇒ ⇒ ⇒ ⇒ 引擎按
   * still_wanted 判是否真退）。**切档不要用它** ✗（那正是本方案要废除的行为）；
   * 只给域级操作（切 wt / 关页面 / 显式释放）与将来的驱逐路径用。
   */
  releaseAll(docId) {
    const sessions = this.sessions.get(docId);
    if (!sessions) return false;
    for (const s of [...sessions]) {
      sessions.delete(s);
      try { s.dispose(); } catch (e) { /* ignore */ }
      this._sendHeaderOnly(Tag.CloseDoc, s._peerId || 0, docId);
    }
    this.sessions.delete(docId);
    const w = this._openWatches.get(docId);
    if (w) { w.dispose(); this._openWatches.delete(docId); }
    return true;
  }


  /**
   * 文件树（含 /.mds，?hidden=true）——**按 wt 作用域**：?wt_uid=<当前 wt>。
   * 不带 wt 时网关只按 Referer 兜底，genesis 页会拿到整棵 VFS（含 /.mds/mounts/** ＝ 别的 wt 的挂载树，
   * 实测 168 条里 90 条是 mounts）⇒ fileview 必须按 wt 取，才「只显示当前 wt 之下的内容」。
   */
  async fullTree(wtUid) {
    const q = "/fs/tree?hidden=true" + (wtUid ? "&wt_uid=" + encodeURIComponent(String(wtUid)) : "");
    const res = await fetch(q);
    if (!res.ok) throw new Error("文件树请求失败 " + res.status);
    return res.json();
  }

  /**
   * 读**指定工作树**下一篇文档的内容（正式通道方法）。
   * 为什么必须走 HTTP 而不是连接：连接自带工作树、且服务端"连接的工作树"优先于载荷
   * （在子工作树里用连接读，会落到该树自己的 /.mds/，读不到全局注册表）。HTTP 那条尊重载荷里的
   * 工作树 ⇒ 只有它读得到别的工作树的文档。fetch 的放行面由源码规则按方法白名单管（见 scripts/judge.mjs）。
   */
  async readDoc(wtUid, mdPath) {
    const res = await fetch("/exec", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ code: "$ " + String(mdPath || ""), active_doc: "/", wt_uid: String(wtUid || "") }),
    });
    const o = await res.json();
    if (!o || o.status !== "ok") throw new Error(String((o && o.result) || "exec 失败"));
    return String(o.result || "");
  }

  /**
   * 读**指定工作树**下一篇文档的 tojson AST（与 readDoc 同一条 HTTP 通道、同一理由：
   * 连接自带工作树 ⇒ 连接 exec 读别的工作树会被连接 wt 覆盖）。
   * 返回已 parse 的 AST envelope；由调用方自己按拓扑取字段（不要在 JS 里重新做 md 文本解析）。
   */
  async readDocAst(wtUid, mdPath) {
    const res = await fetch("/exec", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ code: "$ " + String(mdPath || "") + " | tojson", active_doc: "/", wt_uid: String(wtUid || "") }),
    });
    const o = await res.json();
    if (!o || o.status !== "ok") throw new Error(String((o && o.result) || "exec 失败"));
    return JSON.parse(String(o.result || "{}"));
  }

  /** 当前文档的交付树 = **店本身**（店就是 root 节点；没有这档 ⇒ null）。 */
  tree(docId) {
    return this.wtStore.by.get(String(docId)) || null;
  }

  /** 文档店（波 4）：`{ docId, path, nt, ct }`；没有 ⇒ null。 */
  md(docId) { return this.wtStore.by.get(String(docId)) || null; }

  /** 工作树店（波 4）：`{ uid, by: Map<docId,MD>, entries }`。 */
  wt() { return this.wtStore; }

  /** 当前连接状态。 */
  state() {
    return this._connState;
  }

  /** 帧 bus：每次 F/P 落地后同步回调。 */
  subscribe(cb) {
    this._listeners.add(cb);
    return function () { this._listeners.delete(cb); }.bind(this);
  }

  dispose() {
    this._readLoopRunning = false;
    for (const [, sessions] of this.sessions) for (const s of sessions) s.dispose();
    this.sessions.clear();
    this.wtStore.clearAll();
    this.worker?.postMessage({ type: "dispose" });
    this.worker?.terminate();
    this.worker = null;
    this.writer = null;
    this.reader = null;
    this.isReady = false;
  }
}

// ════════════════════ AST 视图（F/P → 交付树） ════════════════════

/** 文档 id → 文件面 path（通道 onTree 交付的 entries；查不到返回空）。
 *  用途：文档店建店时自带身份 —— path 属于 md 根，之后节点沿 chain 向上继承（不查表、不额外 observe）。 */
function pathFromEntries(client, docId) {
  const es = (client.wtStore && client.wtStore.entries) || [];
  for (const e of es) {
    if (e && e.docId === docId) return String(e.path || "");
  }
  return "";
}

/** 文件面（entries）到达时，把**已建店但 path 为空**的文档店补上（一个树事件一次循环，不是每节点）。
 *  这样「帧先到、树后到」的时序也不会让 md 根丢身份；文档店是 MST 节点，setPath 通知会让认领判定重算。 */
function backfillPaths(client, entries) {
  if (!Array.isArray(entries)) return;
  for (const e of entries) {
    if (!e || typeof e.docId !== "number") continue;
    const md = client.md(e.docId);
    if (!md || md.path) continue;
    try { md.setPath(String(e.path || "")); } catch (err) { /* 单篇失败不影响其余 */ }
  }
}

function createAstView(client, docId, silent) {
  const rcPeerId = client.peerId;   // 步骤 4：与 Monaco 同属本连接的**同一个** peer
  let resync = false;
  const notify = (type, payload) => {
    const rev = (client._rev.get(docId) || 0);
    // 事件在**边界**组装：视图层需要什么由这里一次给全（热路径不打点，判官 R4b 保持）
    const _now2 = (typeof performance !== "undefined" && performance.now) ? performance.now() : Date.now();
    // 锚点只在一个短窗口内有效：一次写常来多帧（TextOp + Ast）⇒ 窗口内每帧都归属它；
    // 超出窗口（例如下一帧隔了很久）⇒ 判为陈旧，incr 记 null 而不是报出 1 秒级的假值（实测踩过）。
    const _age = client._wasmInAt == null ? Infinity : (_now2 - client._wasmInAt);
    const _fresh = _age <= 250;
    const ev = { docId, type, rev, tree: client.tree(docId), ops: payload && payload.ops ? payload.ops : [], bytes: payload && payload.bytes ? payload.bytes : 0, wasmMs: _fresh ? Math.round(_age * 10) / 10 : null, wasmWallIn: _fresh ? (client._wasmInWall || null) : null, wasmWallOut: _fresh ? Date.now() : null };
    // 观测口：本轮由本地发送触发的帧，其墙钟两端直接进 console（便于外部计时对齐，不在日志面板抓）
    if (ev.wasmWallIn != null) { try { console.log("[INC] type=" + type + " doc=" + docId + " wall=" + ev.wasmWallIn + "->" + ev.wasmWallOut + " wasmMs=" + ev.wasmMs); } catch (e) { /* ignore */ } }
    for (const cb of Array.from(client._listeners)) {
      try { cb(ev); } catch (e) { console.error("[client] 帧订阅者抛错", e); }
    }
  };
  return {
    _peerId: rcPeerId,
    dispose() {},
    handleMessage(tag, data) {
      if (data.length < 19) return;
      const st = data[13];
      if (st !== AST_FULL && st !== AST_PATCH) return;
      const dv = new DataView(data.buffer, data.byteOffset, data.byteLength);
      const jsonLen = dv.getUint32(14, true);
      let json;
      try {
        json = JSON.parse(new TextDecoder().decode(data.subarray(18, 18 + jsonLen)));
      } catch (e) {
        console.warn("[client] 帧 JSON 解析失败:", e);
        return;
      }
      if (st === AST_FULL) {
        // 重同步（失败恢复时自己要来的真相）必须收下；否则树非空时的重复全量帧丢弃。
        const take = resync || !client.wtStore.by.has(String(docId));
        if (resync) resync = false;
        if (!take) return;
        // 建店（或按**同键调和**复用旧店）⇒ 里面换一棵新 root（F 帧语义 = 换新实例；R7.1 已钉）
        // path 属于 md 根：open 之外，经 observe/帧装载的文档也要自带身份（建店时从通道文件面补一次；之后继承）。
        const prevPath = (client.md(docId) || {}).path || "";
        client.wtStore.put(docId, { docId, path: prevPath || pathFromEntries(client, docId), ...json });
        client.wtStore.note(docId, "ready");   // F 帧落地 ⇒ ready（阶段①-3 写点）
        client._rev.set(docId, (client._rev.get(docId) || 0) + 1);
        notify("snapshot", { bytes: jsonLen });
        return;
      }
      // P：MST 自带 applyPatch 直接吃 op 数组（模型原地应用并通知 observer）；
      // 失败 = 大声报 + 请求全量重同步（同 peer 重发 UP_OPEN）
      try {
        if (!Array.isArray(json)) throw new Error("P 帧载荷不是 op 数组");
        const md = client.md(docId);
        if (md == null) throw new Error("P 帧先于 F 帧（本地无模型）");
        assertPatchOps(json);
        applyPatch(md, json);   // 补丁路径是**文档树内**的路径；店就是 root 节点 ⇒ 打在店上（原地，保实例）
        client._rev.set(docId, (client._rev.get(docId) || 0) + 1);
        notify("patch", { ops: json, bytes: jsonLen });
      } catch (e) {
        resync = true;
        console.error("[client] 补丁应用失败 → 请求全量重同步 err=" + String((e && e.message) || e).slice(0, 200) + " ops=" + JSON.stringify(json).slice(0, 200));
        try { client._sendOpenDoc(rcPeerId, docId, Tag.OpenDoc, MODE_AST); } catch (e2) { /* 连接已断时忽略 */ }
      }
    },
  };
}

/** 装配口：起 worker + 连接网关。返回 client（exec/open/close/reopen/tree/state/subscribe）。 */
export async function connectClient(url, opts) {
  const client = new MdioClient(opts || {});
  await client.init();
  await client.connect(url);
  return client;
}