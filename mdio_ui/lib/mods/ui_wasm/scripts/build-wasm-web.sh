#!/usr/bin/env bash
set -euo pipefail
# wasm web 适配器的独立编译链（引擎多目标之一）：
#   cargo 打 crate（lib/wasm）→ wasm-bindgen 产进 pkg → 快照进 src/wasm → esmx 构建 ui_wasm（原子发布）。
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd -- "$SCRIPT_DIR/../../../../.." && pwd)"
source "$ROOT/mdio_ui/lib/wasm/toolchain.env"
cd "$ROOT"
command -v wasm-bindgen >/dev/null 2>&1 || { echo "wasm-bindgen ${WASM_BINDGEN_VERSION} required; run bash mdio_ui/lib/wasm/scripts/bootstrap-wasm-tools.sh" >&2; exit 1; }
ACTUAL_WASM_BINDGEN="$(wasm-bindgen --version | awk '{print $2}')"
[ "$ACTUAL_WASM_BINDGEN" = "$WASM_BINDGEN_VERSION" ] || { echo "wasm-bindgen version mismatch: expected ${WASM_BINDGEN_VERSION}, got ${ACTUAL_WASM_BINDGEN}" >&2; exit 1; }
rustup target list --installed | grep -qx wasm32-unknown-unknown || { echo "missing rust target wasm32-unknown-unknown; run bash mdio_ui/lib/wasm/scripts/bootstrap-wasm-tools.sh" >&2; exit 1; }
cargo build --manifest-path mdio_ui/lib/wasm/Cargo.toml --target wasm32-unknown-unknown --release --no-default-features
wasm-bindgen target/wasm32-unknown-unknown/release/editor_core.wasm --out-dir mdio_ui/lib/wasm/pkg --target web --no-typescript
bash mdio_ui/lib/mods/ui_wasm/scripts/sync-pkg.sh
if [ "${MDIO_WASM_SKIP_UI_BUILD:-0}" != "1" ]; then
  node mdio_ui/demo/scripts/build-wt-ui.mjs mdio_ui/lib/mods/ui_wasm --force
fi
node mdio_ui/lib/mods/ui_wasm/scripts/stamp-engine.mjs
echo ui_wasm-chain-done
