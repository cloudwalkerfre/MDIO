#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "$0")" && pwd)"
ROOT="$(cd -- "$SCRIPT_DIR/../.." && pwd)"
cd "$ROOT"

# 打包输入用**已提交的** demo 清单：本地 dev 的 `sync-wt-uses` 会把 worktrees 里的 wt ui 包
# 追加成 `link:` 依赖（供 dev 挂载），这些本地链接不在 lockfile 里 ⇒ `pnpm install
# --frozen-lockfile` 直接 ERR_PNPM_OUTDATED_LOCKFILE（2026-11-02 实测）。构建期间临时换成
# HEAD 版，退出时还原，不污染开发者的工作区。
DEMO_PKG="$ROOT/mdio_ui/demo/package.json"
PKG_BAK="$(mktemp)"
cp "$DEMO_PKG" "$PKG_BAK"
trap 'cp "$PKG_BAK" "$DEMO_PKG"; rm -f "$PKG_BAK"' EXIT
git -C "$ROOT" show HEAD:mdio_ui/demo/package.json > "$DEMO_PKG"

docker run --rm -v "$ROOT":/work -w /work node:24.21.0-bookworm bash -lc '
set -euo pipefail
export CI=true
corepack enable
corepack prepare pnpm@11.28.5 --activate
pnpm --dir mdio_ui/demo install --frozen-lockfile
for d in ui_base ui_wasm ui_core ui_appview ui_devview ui_std ui_router; do
  if [ -f "mdio_ui/lib/mods/$d/pnpm-lock.yaml" ]; then
    pnpm --dir "mdio_ui/lib/mods/$d" install --ignore-workspace --frozen-lockfile
  else
    pnpm --dir "mdio_ui/lib/mods/$d" install --ignore-workspace --no-frozen-lockfile
  fi
done
cd mdio_ui/demo
node scripts/build-wt-ui.mjs /work/mdio_ui/lib/mods/ui_base --force
for d in ui_wasm ui_core ui_appview ui_devview ui_std ui_router; do
  node scripts/build-wt-ui.mjs "/work/mdio_ui/lib/mods/$d" --force
done
pnpm build
'

WEB="$ROOT/mdio_py/src/mdio_py/web/client"
rm -rf "$WEB"
mkdir -p "$WEB/demo"
cp "$ROOT/mdio_ui/demo/dist/client/index.html" "$WEB/index.html"
cp -R "$ROOT/mdio_ui/demo/dist/client/." "$WEB/demo/"
for m in ui_base ui_core ui_appview ui_devview ui_std ui_router ui_wasm; do
  mkdir -p "$WEB/$m"
  cp -R "$ROOT/mdio_ui/lib/mods/$m/dist/client/." "$WEB/$m/"
done
# /vendor/* 是运行期直出（mdLine.js 的 JSX 转换器从这里取）：dev 宿主由 demo/src/entry.node.ts
# 的 `/vendor/*` 路由提供；打包后静态根 = web/client，所以必须拷进 $WEB/vendor，否则 8091 上
# /vendor/babel-standalone.mjs 404 ⇒ 所有 md 代码件装载失败。
mkdir -p "$WEB/vendor"
cp "$ROOT/mdio_ui/demo/vendor/babel-standalone.mjs" "$ROOT/mdio_ui/demo/vendor/LICENSE-babel.txt" "$WEB/vendor/"
echo "assembled UI web root: $WEB"
