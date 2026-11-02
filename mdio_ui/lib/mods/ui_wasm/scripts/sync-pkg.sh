#!/usr/bin/env bash
set -euo pipefail
# 快照 lib/wasm/pkg（wasm-bindgen 封装）→ ui_wasm/src/wasm/。
# 构建产物不提交；cargo 重刷 = 手动 just build-wasm 后再跑本脚本。
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd -- "$SCRIPT_DIR/../../../../.." && pwd)"
cd "$ROOT"
rm -rf mdio_ui/lib/mods/ui_wasm/src/wasm
mkdir -p mdio_ui/lib/mods/ui_wasm/src/wasm
cp mdio_ui/lib/wasm/pkg/editor_core.js mdio_ui/lib/mods/ui_wasm/src/wasm/
cp mdio_ui/lib/wasm/pkg/editor_core_bg.wasm mdio_ui/lib/mods/ui_wasm/src/wasm/
cp -R mdio_ui/lib/wasm/pkg/snippets mdio_ui/lib/mods/ui_wasm/src/wasm/
echo pkg-snapshot-done
