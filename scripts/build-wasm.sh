#!/bin/sh
# Build the engine and analysis WebAssembly modules into web/public/wasm.
# With --if-available, skip quietly when cargo is not installed: the built
# modules are committed so the prototypes run without a Rust toolchain.
set -e
cd "$(dirname "$0")/.."
if ! command -v cargo >/dev/null 2>&1; then
  if [ "$1" = "--if-available" ]; then
    echo "cargo not found; using the committed web/public/wasm/*.wasm"
    exit 0
  fi
  echo "cargo not found. Install Rust (https://rustup.rs) and run: rustup target add wasm32-unknown-unknown" >&2
  exit 1
fi
cargo build --release --target wasm32-unknown-unknown -p engine -p analysis
mkdir -p web/public/wasm
cp target/wasm32-unknown-unknown/release/engine.wasm web/public/wasm/engine.wasm
cp target/wasm32-unknown-unknown/release/analysis.wasm web/public/wasm/analysis.wasm
ls -l web/public/wasm
