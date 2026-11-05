# MDIO development entrypoints. Every build/test/release action goes through build_grid.

set dotenv-load

python_version := "3.14"
grid := "python3 tools/build_grid/runner.py"

default:
    @just --list

# ==========================================
# Setup / toolchain
# ==========================================

doctor:
    {{grid}} doctor

install:
    uv sync --python {{python_version}}
    {{grid}} ui:wasm

# ==========================================
# Build
# ==========================================

build-arrow:
    {{grid}} py:arrow

build-wasm:
    {{grid}} ui:wasm

build-ui:
    {{grid}} ui:bundle

build:
    {{grid}} build

wheel:
    {{grid}} wheel

verify:
    {{grid}} verify

release:
    {{grid}} release

clean:
    {{grid}} clean

# ==========================================
# Test / gates
# ==========================================

test:
    {{grid}} test

test-rust:
    {{grid}} rust:test

test-py:
    {{grid}} py:test

test-py-slow:
    {{grid}} py:test-slow

test-omp:
    {{grid}} test:omp

test-trinity:
    {{grid}} test:trinity

# 重新生成 mdio_core/tests/unit/ledger_report.txt（与 ledger_report_fixture_is_current 同源）
regen-ledger-report:
    cargo run -q -p mdio_core --features trinity-obs --example regen_ledger_report

# 对照当前代码与已提交夹具（夹具过期则红）
check-ledger-report:
    cargo test -q -p mdio_core --lib --features trinity-obs ledger_report_fixture_is_current

check:
    {{grid}} check

check-layers:
    {{grid}} check:layers

check-core-layers:
    {{grid}} check:core-layers

# ==========================================
# Development
# ==========================================

serve:
    {{grid}} dev:serve

demo:
    {{grid}} dev:demo

kit-build:
    {{grid}} kit:build
