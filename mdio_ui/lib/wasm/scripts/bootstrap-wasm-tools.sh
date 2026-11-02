#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd -- "$SCRIPT_DIR/../../../.." && pwd)"
source "$ROOT/mdio_ui/lib/wasm/toolchain.env"

cd "$ROOT"
rustup toolchain install "$RUST_TOOLCHAIN" --profile minimal
rustup target add wasm32-unknown-unknown --toolchain "$RUST_TOOLCHAIN"
cargo +"$RUST_TOOLCHAIN" install wasm-bindgen-cli --version "$WASM_BINDGEN_VERSION" --locked
echo "wasm tools ready: rust $RUST_TOOLCHAIN, wasm-bindgen $WASM_BINDGEN_VERSION"
