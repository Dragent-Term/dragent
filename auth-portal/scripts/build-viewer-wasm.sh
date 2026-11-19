#!/usr/bin/env bash
# Build the shared-session web viewer WASM bundle (the Dragent desktop app
# compiled to WebAssembly) and place the browser bindings in public/wasm/.
#
# Output: public/wasm/dragent_web_viewer{.js,_bg.wasm,.d.ts,_bg.wasm.d.ts}
# Loaded by src/components/WasmViewer.tsx at /join.
set -euo pipefail

portal_root="$(cd "$(dirname "$0")/.." && pwd)"
repo_root="$(cd "$portal_root/.." && pwd)"
dragent_root="$repo_root/dragent"
output_dir="$portal_root/public/wasm"
out_name="dragent_web_viewer"
wasm_out="$output_dir/${out_name}_bg.wasm"

# In CI without a Rust toolchain, fall back to the checked-in artifacts (same
# contract as build-terminal-wasm.sh) so the Next.js build can still proceed.
if ! command -v cargo >/dev/null 2>&1 || ! command -v wasm-bindgen >/dev/null 2>&1; then
  if [[ -f "$output_dir/${out_name}.js" && -f "$wasm_out" ]]; then
    echo "Rust toolchain unavailable; using checked-in WASM artifacts."
    exit 0
  fi
  echo "cargo and wasm-bindgen are required to build the viewer WASM artifact." >&2
  exit 1
fi

echo "==> cargo build --release (wasm32) — this recompiles the app optimized"
cargo build --release --manifest-path "$dragent_root/Cargo.toml" \
  -p dragent_web_viewer --target wasm32-unknown-unknown

echo "==> wasm-bindgen (browser bindings)"
wasm-bindgen \
  "$dragent_root/target/wasm32-unknown-unknown/release/${out_name}.wasm" \
  --target web --out-dir "$output_dir" --out-name "$out_name"

# Optional extra shrink. Install with `brew install binaryen` (or
# `cargo install wasm-opt`). Skipped automatically when not present.
if command -v wasm-opt >/dev/null 2>&1; then
  echo "==> wasm-opt -Oz --strip-debug (size/perf pass; drops DWARF debuginfo)"
  wasm-opt -Oz --strip-debug --enable-bulk-memory --enable-nontrapping-float-to-int \
    "$wasm_out" -o "$wasm_out"
else
  echo "==> wasm-opt not found — skipping (install binaryen for a smaller bundle)"
fi

# A tiny no-cache manifest lets the join page give both generated assets the
# same content-hash query string. This prevents a browser/CDN from pairing a new
# JS binding with an old 19MB wasm binary (or serving the previous UI entirely).
bundle_version="$(shasum -a 256 "$wasm_out" | cut -c1-16)"
printf '%s\n' "$bundle_version" > "$output_dir/version.txt"

# Theme background images are NOT embedded in the wasm (they'd add ~17MB) — the
# viewer fetches them by URL from /wasm-themes/. Publish them next to the bundle.
themes_src="$dragent_root/crates/dragent_viewer/assets/themes"
themes_dst="$portal_root/public/wasm-themes"
echo "==> publishing theme images → public/wasm-themes/"
rm -rf "$themes_dst"
mkdir -p "$themes_dst"
rsync -a --prune-empty-dirs \
  --include='*/' --include='*.jpg' --include='*.png' --exclude='*' \
  "$themes_src/" "$themes_dst/"

size="$(du -h "$wasm_out" | cut -f1)"
themes_size="$(du -sh "$themes_dst" | cut -f1)"
echo "==> done: $wasm_out ($size) + public/wasm-themes ($themes_size)"
