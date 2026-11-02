// @ts-check
/**
 * mdScope —— wt/app 局域的 md module registry（纯注入，零框架 import）。
 *
 * 语义：
 *   - bindings: name -> { name, addr }；由 modLoader.beginScope(decl) 从 decl.mods 的 .md:: 项派生；
 *   - load(name): 只在当前 scope 内解析；同名 promise 去重；依赖递归由 loadModule 回传的 loadDep 完成；
 *   - 本 scope 的模块只能被本 scope 的 loadModule 使用；不写页面 importmap。
 *
 * 生命周期：
 *   - UiDecl.reload 每轮 beginScope 建新 scope，旧 scope dispose；
 *   - dispose 后在飞的 load 结果丢弃，不写新 decl 的 mods。
 */

export function createMdScope(opts) {
  const o = opts || {};
  const bindings = o.bindings instanceof Map ? o.bindings : new Map();
  const loadModule = o.loadModule;
  const log = typeof o.log === "function" ? o.log : function () {};

  const cache = new Map();      // name -> record
  const loading = new Map();    // name -> promise
  let disposed = false;
  let gen = 0;

  function err(msg) {
    return new Error(msg);
  }

  function load(name, chain) {
    const stack = Array.isArray(chain) ? chain : [];
    if (disposed) return Promise.reject(err("md scope disposed: " + name));
    if (!bindings.has(name)) {
      const from = stack.length ? " (from " + stack.join(" -> ") + ")" : "";
      return Promise.reject(err("local module not found: @app/" + name + from));
    }
    if (stack.indexOf(name) >= 0) {
      return Promise.reject(err("local module cycle: " + stack.concat(name).join(" -> ")));
    }
    if (cache.has(name)) return Promise.resolve(cache.get(name));
    if (loading.has(name)) return loading.get(name);
    if (typeof loadModule !== "function") {
      return Promise.reject(err("md scope: no loadModule injected"));
    }

    const next = stack.concat(name);
    const myGen = gen;
    const p = Promise.resolve()
      .then(function () {
        if (disposed || myGen !== gen) throw err("md scope disposed: " + name);
        const rec = loadModule(name, bindings.get(name).addr, function (dep) {
          return load(dep, next);
        });
        return Promise.resolve(rec);
      })
      .then(function (rec) {
        if (disposed || myGen !== gen) throw err("md scope disposed: " + name);
        cache.set(name, rec);
        log("md-scope: loaded @app/" + name + (rec && rec.url ? " · " + String(rec.url).slice(0, 48) : ""));
        return rec;
      })
      .finally(function () {
        loading.delete(name);
      });

    loading.set(name, p);
    return p;
  }

  return {
    has(name) { return bindings.has(String(name)); },
    load(name) { return load(String(name), []); },
    dispose() {
      disposed = true;
      gen += 1;
      cache.clear();
      loading.clear();
    },
  };
}
