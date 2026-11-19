#!/usr/bin/env bash
set -euo pipefail

portal_root="$(cd "$(dirname "$0")/.." && pwd)"
repo_root="$(cd "$portal_root/.." && pwd)"
dragent_root="$repo_root/dragent"
output_dir="$portal_root/public/wasm"

if ! command -v cargo >/dev/null 2>&1 || ! command -v wasm-bindgen >/dev/null 2>&1; then
  if [[ -f "$output_dir/dragent_terminal_wasm.js" && -f "$output_dir/dragent_terminal_wasm_bg.wasm" ]]; then
    echo "Rust toolchain unavailable; using checked-in WASM artifacts."
    exit 0
  fi
  echo "cargo and wasm-bindgen are required to build the terminal WASM artifact." >&2
  exit 1
fi

cargo build --release --manifest-path "$dragent_root/Cargo.toml" \
  -p dragent_terminal_wasm --target wasm32-unknown-unknown
wasm-bindgen \
  "$dragent_root/target/wasm32-unknown-unknown/release/dragent_terminal_wasm.wasm" \
  --target web --out-dir "$output_dir" --out-name dragent_terminal_wasm
