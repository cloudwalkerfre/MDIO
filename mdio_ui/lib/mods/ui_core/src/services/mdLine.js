// @ts-check
/**
 * mdLine —— 唯一自写装载线（§2.2）：md 代码块 = exec 读文本 → Blob → import。
 * ui/npm 件不走这里（浏览器原生 ESM + import map，esmx 注入）。零 import，exec 注入。
 *
 * **这条路就是原样**（不要再起第二条通路）。
 * 唯一新增的一步是 `transformIfJsx`：源码看起来像 JSX 时，在**同一处**编译一次再进 Blob。
 * 不需要 JSX 的话，把 `await transformIfJsx(...)` 换成 `src` 即可回到完全原样。
 */

// —— JSX：Blob 装载没有编译期 ⇒ 运行时转一次（只在源码像 JSX 时才加载 transformer，加载后缓存）——
// 用 automatic runtime：产出 `import { jsx } from "react/jsx-runtime"`，该裸标识符在宿主 import map 里 ⇒ React 单例不破、hooks 正常。
const JSX_HINT = /<\/?[A-Za-z][\w.$:-]*(\s|\/|>)|<>|<\/>/;
let _jsx = null;

// 编译结果缓存：声明重装（改 ui.md）会把全部 md 件重新走一遍「读文本 → JSX 编译 → @app 改写」。
// 内容没变时这一步是纯函数 ⇒ 按 (地址, 源码[, 依赖替换]) 记下结果，重装只花 exec 读文本。
// 实测：genesis 树 31 个 md 件，无缓存时一次声明热重载 ≈ 0.5–0.7s；命中缓存后只做 exec 读取。
const COMPILE_CACHE_MAX = 800;
function cachePut(map, key, val) {
  if (map.size >= COMPILE_CACHE_MAX) map.delete(map.keys().next().value);
  map.set(key, val);
  return val;
}
const jsxCodeCache = new Map();       // label + "\0" + src -> 编译后的 code
const scopedDepsCache = new Map();    // label + "\0" + src -> @app/ 依赖名列表
const scopedCodeCache = new Map();    // label + "\0" + src + "\0" + replacements -> 改写后的 code

async function jsxTransformer() {
  if (_jsx) return _jsx;
  // **本地 vendor**（实测改）：原先是 esm.sh 的 CDN 依赖 —— 浏览器取它约 10 秒且经常失败，
  // 换树首屏被拖到 10.6s，且 jsx@page 的 md 件静默缺件（decl ✗ … 取件失败）。与 monaco 同款：
  // `mdio_ui/demo/vendor/babel-standalone.mjs`（自包含 ESM）由壳的 `/vendor/` 路由直出；刷新用
  // `node scripts/build-babel.mjs`（钉版本 + 校验哈希）。
  const mod = await import(/* @vite-ignore */ "/vendor/babel-standalone.mjs");
  const B = mod && (mod.default || mod);
  if (!B || typeof B.transform !== "function") throw new Error("JSX transformer 不可用");
  _jsx = B;
  return B;
}

async function transformIfJsx(src, label) {
  if (!JSX_HINT.test(src)) return src;              // 不含 JSX ⇒ 原样返回（纯 JS 块零代价）
  const key = label + "\u0000" + src;
  const hit = jsxCodeCache.get(key);
  if (hit !== undefined) return hit;
  const B = await jsxTransformer();
  const out = B.transform(src, { filename: label, sourceType: "module", presets: [["react", { runtime: "automatic" }]] });
  return cachePut(jsxCodeCache, key, String(out.code));
}

/** 读一个 md 位置的文本（装载线与**内容判据**共用这一条读法：读法只此一处）。 */
export async function readMdText(addr, exec) {
  const a = String(addr == null ? "" : addr).trim();
  if (!a) throw new Error("md 装载：空地址");
  return String(await exec("$ " + a + (a.indexOf("::") >= 0 ? " -pj ct" : "")));
}

/**
 * 已装载的 md 件按**内容**缓存：代码也是数据 ⇒ 内容没变就还是同一个模块。
 * 为什么必须（实测）：重新 import 一个 Blob ⇒ **新的模块身份** ⇒ 组件函数换引用 ⇒ React 卸载重挂；
 * 每次数据写都这样重来一遍 ⇒ 组件状态复位、"数字+1"要等重挂才可见（§16A 根因 A 的第二半）。
 * 以编译后的代码文本为键：未变 ⇒ 取回**同一个命名空间**（不换身份、不 remount）；变了 ⇒ 新键 ⇒ 新模块。
 * 也是 §16A 提案第 4 条的"兜底"：即使判据误触发重装，未变的件也不会换身份。
 */
const nsByCode = new Map();

export async function loadMd(name, addr, exec) {
  const src = await readMdText(addr, exec);
  const code = await transformIfJsx(src, addr);
  const hit = nsByCode.get(code);
  if (hit) return { name, ns: hit, url: "md:cache(" + name + ")", via: "md", src };
  const blob = URL.createObjectURL(new Blob([code], { type: "text/javascript" }));
  try {
    const ns = await import(/* @vite-ignore */ blob);
    nsByCode.set(code, ns);
    return { name, ns, url: blob, via: "md", src };
  } finally {
    // 装载完成后即可释放（模块已取到，身份由 nsByCode 持有）；与老壳同口径
    try { URL.revokeObjectURL(blob); } catch (e) { /* ignore */ }
  }
}

// ── wt-scope local md modules（@app/*）────────────────────────────────────────
// 与 loadMd 分开：普通 md 件仍 exec→Babel→Blob→revoke；scope 件的依赖 URL 必须保留，
// 否则父模块 Blob 的静态 import 会在 fetch 时失败（实测）。
const scopedByCode = new Map();   // finalCode -> { ns, url }

function collectScopedPlugin(sinks) {
  return function () {
    return {
      visitor: {
        ImportDeclaration(p) {
          const s = p.node && p.node.source && p.node.source.value;
          if (typeof s === "string" && s.indexOf("@app/") === 0) sinks.deps.push(s);
        },
        ExportNamedDeclaration(p) {
          const s = p.node && p.node.source && p.node.source.value;
          if (typeof s === "string" && s.indexOf("@app/") === 0) sinks.deps.push(s);
        },
        ExportAllDeclaration(p) {
          const s = p.node && p.node.source && p.node.source.value;
          if (typeof s === "string" && s.indexOf("@app/") === 0) sinks.deps.push(s);
        },
        ImportExpression(p) {
          const s = p.node && p.node.source;
          if (s && s.type === "StringLiteral" && typeof s.value === "string" && s.value.indexOf("@app/") === 0) {
            sinks.dynamic.push(s.value);
          }
        },
        CallExpression(p) {
          // Babel 7 standalone 把动态 import 表达成 CallExpression(callee.type === "Import")，
          // 不是 ImportExpression；两条都收，防止版本差异漏检。
          const c = p.node && p.node.callee;
          if (!c || c.type !== "Import") return;
          const a = p.node.arguments && p.node.arguments[0];
          if (a && a.type === "StringLiteral" && typeof a.value === "string" && a.value.indexOf("@app/") === 0) {
            sinks.dynamic.push(a.value);
          }
        },
      },
    };
  };
}

function rewriteScopedPlugin(replacements) {
  function repl(node) {
    const s = node && node.source && node.source.value;
    if (typeof s === "string" && Object.prototype.hasOwnProperty.call(replacements, s)) {
      node.source.value = replacements[s];
    }
  }
  return function () {
    return {
      visitor: {
        ImportDeclaration(p) { repl(p.node); },
        ExportNamedDeclaration(p) { repl(p.node); },
        ExportAllDeclaration(p) { repl(p.node); },
      },
    };
  };
}

function scopedSinks() {
  return { deps: [], dynamic: [] };
}

async function collectScopedImports(src, label) {
  const key = label + "\u0000" + src;
  const hit = scopedDepsCache.get(key);
  if (hit !== undefined) return hit;
  const B = await jsxTransformer();
  const sinks = scopedSinks();
  B.transform(src, {
    filename: label,
    sourceType: "module",
    plugins: [collectScopedPlugin(sinks)],
    presets: [["react", { runtime: "automatic" }]],
    code: false,
    ast: false,
  });
  if (sinks.dynamic.length) {
    throw new Error("local dynamic import is not supported in md scope: " + sinks.dynamic.join(", "));
  }
  return cachePut(scopedDepsCache, key, sinks.deps);
}

async function rewriteScopedImports(src, label, replacements) {
  const key = label + "\u0000" + src + "\u0000" + JSON.stringify(replacements);
  const hit = scopedCodeCache.get(key);
  if (hit !== undefined) return hit;
  const B = await jsxTransformer();
  const out = B.transform(src, {
    filename: label,
    sourceType: "module",
    plugins: [rewriteScopedPlugin(replacements)],
    presets: [["react", { runtime: "automatic" }]],
    ast: false,
  });
  return cachePut(scopedCodeCache, key, String(out.code));
}

/**
 * 装载一个 scope 内的 md 件：先收集 @app/* 静态依赖，递归拿依赖 URL，再改写并编译。
 * deps.loadDep(spec) -> Promise<{ url }>；本函数只由 mdScope.loadModule 注入调用。
 */
export async function loadScopedMd(name, addr, exec, loadDep) {
  const src = await readMdText(addr, exec);
  const hasLocal = src.indexOf("@app/") >= 0;
  const hasJsx = JSX_HINT.test(src);

  let code = src;
  if (hasLocal) {
    const deps = await collectScopedImports(src, addr);
    const uniq = [];
    for (let i = 0; i < deps.length; i++) if (uniq.indexOf(deps[i]) < 0) uniq.push(deps[i]);
    const replacements = {};
    for (let i = 0; i < uniq.length; i++) {
      const spec = uniq[i];
      const depName = spec.indexOf("@app/") === 0 ? spec.slice(5) : spec;
      const dep = await loadDep(depName);
      if (!dep || typeof dep.url !== "string" || !dep.url) {
        throw new Error("local module has no URL: " + spec + " (from " + name + ")");
      }
      replacements[spec] = dep.url;
    }
    code = await rewriteScopedImports(src, addr, replacements);
  } else if (hasJsx) {
    code = await transformIfJsx(src, addr);
  }

  const hit = scopedByCode.get(code);
  if (hit) return { name, ns: hit.ns, url: hit.url, via: "md-scope", src };

  const blob = URL.createObjectURL(new Blob([code], { type: "text/javascript" }));
  try {
    const ns = await import(/* @vite-ignore */ blob);
    scopedByCode.set(code, { ns, url: blob });
    return { name, ns, url: blob, via: "md-scope", src };
  } catch (e) {
    try { URL.revokeObjectURL(blob); } catch (e2) { /* ignore */ }
    throw e;
  }
}
