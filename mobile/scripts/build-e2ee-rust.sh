#!/usr/bin/env bash
# Builds the phone's native E2EE library (dragent_e2ee through uniffi) for the
# Expo module in mobile/modules/dragent-e2ee (docs/design/e2ee.md §3.2, E4).
#
#   build-e2ee-rust.sh pins             check the §3.2 dependency pins only
#   build-e2ee-rust.sh bindings         regenerate the Swift/Kotlin bindings (committed)
#   build-e2ee-rust.sh check-bindings   fail if the committed bindings are stale
#   build-e2ee-rust.sh ios              build ios/Frameworks/DragentE2EEFFI.xcframework (device + simulator)
#   build-e2ee-rust.sh android          build android/src/main/jniLibs/<abi>/libdragent_e2ee_ffi.so
#   build-e2ee-rust.sh all              pins, check-bindings, ios (on macOS), android (when the NDK is set)
#   build-e2ee-rust.sh eas              EAS `eas-build-pre-install` hook: install Rust if the
#                                       image lacks it, then build for EAS_BUILD_PLATFORM
#
# Options:
#   --vectors   compile the Noise/codec vector runners in (simulator tests only;
#               debug profile: the crate refuses them in optimised builds).
#               Output goes to build/vectors/ so it can never be linked into the
#               app; refused when EAS_BUILD_PROFILE is set.
#   --debug     debug profile (default: release, LTO).
#
# Android needs ANDROID_NDK_HOME (or ANDROID_NDK_ROOT) and `cargo ndk`
# (cargo install cargo-ndk --version 4.1.2 --locked).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
module="$here/../modules/dragent-e2ee"
crate="$module/rust"
manifest="$crate/Cargo.toml"
lib="dragent_e2ee_ffi"
# Shared with the desktop workspace's build output to save disk.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$(cd "$here/../../dragent" && pwd)/target}"
ios_min="16.4" # ExpoModulesCore's iOS minimum
android_api="28" # §6.7: setUnlockedDeviceRequired needs API 28

cmd="${1:-all}"
shift || true
profile="release"
vectors=0
for arg in "$@"; do
  case "$arg" in
    --vectors) vectors=1 ;;
    --debug) profile="debug" ;;
    *) echo "build-e2ee-rust: unknown option $arg" >&2; exit 2 ;;
  esac
done

# The vector runners compile only into debug builds (the crate refuses them
# with optimisations on), so --vectors implies --debug.
[ "$vectors" = 1 ] && profile="debug"
if [ "$vectors" = 1 ] && [ -n "${EAS_BUILD_PROFILE:-}" ]; then
  echo "build-e2ee-rust: --vectors is for simulator tests only, never an EAS build" >&2
  exit 2
fi

cargo_flags=(--manifest-path "$manifest" --locked --lib)
[ "$profile" = release ] && cargo_flags+=(--release)
[ "$vectors" = 1 ] && cargo_flags+=(--features vectors)
out_root="$module/build"
[ "$vectors" = 1 ] && out_root="$module/build/vectors"

need() {
  command -v "$1" >/dev/null 2>&1 || { echo "build-e2ee-rust: '$1' is required ($2)" >&2; exit 2; }
}

# §3.2 pins, checked against this crate's own lock file: the exact versions,
# snow's feature set, no ring/blake2/aes-gcm, and dragent_e2ee itself linking
# only getrandom 0.3.4.
check_pins() {
  need cargo "install Rust from https://rustup.rs"
  local tree normal fail=0
  normal="$(cargo tree --manifest-path "$manifest" --locked -e normal --prefix none --target all 2>/dev/null | awk '{print $1" "$2}' | sort -u)"
  for pin in "snow v0.10.0" "x25519-dalek v2.0.1" "chacha20poly1305 v0.10.1" \
             "curve25519-dalek v4.1.3" "hkdf v0.12.4" "sha2 v0.10.9" "zeroize v1.8.2" \
             "getrandom v0.3.4" "uniffi v0.32.2"; do
    printf '%s\n' "$normal" | grep -qx "$pin" || { echo "pins: missing $pin" >&2; fail=1; }
  done
  for c in snow x25519-dalek chacha20poly1305 curve25519-dalek hkdf sha2 zeroize; do
    n="$(printf '%s\n' "$normal" | awk -v c="$c" '$1==c' | wc -l | tr -d ' ')"
    [ "$n" = 1 ] || { echo "pins: $c resolves to $n versions" >&2; fail=1; }
  done
  if printf '%s\n' "$normal" | awk '{print $1}' | grep -Eqx 'ring|blake2|aes-gcm|aes|p256|pqcrypto-.*'; then
    echo "pins: banned crypto crate in the tree" >&2; fail=1
  fi
  tree="$(cargo tree --manifest-path "$manifest" --locked -p dragent_e2ee -e normal --prefix none --target all 2>/dev/null | awk '$1=="getrandom"{print $2}' | sort -u)"
  [ "$tree" = "v0.3.4" ] || { echo "pins: dragent_e2ee must link only getrandom v0.3.4, found: $tree" >&2; fail=1; }
  local feats
  feats="$(cargo tree --manifest-path "$manifest" --locked -e features --prefix none 2>/dev/null \
    | sed -n 's/^snow feature "\([^"]*\)".*/\1/p' | sort -u)"
  if printf '%s\n' "$feats" | grep -Evq '^(use-curve25519|use-chacha20poly1305|use-sha2|default-resolver|curve25519-dalek|chacha20poly1305|sha2)$'; then
    echo "pins: snow has features outside the §3.2 pin: $(echo $feats)" >&2; fail=1
  fi
  [ "$fail" = 0 ] || exit 1
  echo "pins: ok"
}

host_dylib() {
  local feat=()
  [ "$vectors" = 1 ] && feat=(--features vectors)
  cargo build --manifest-path "$manifest" --locked --lib "${feat[@]+"${feat[@]}"}" >&2
  local d="$CARGO_TARGET_DIR/debug"
  if [ -f "$d/lib$lib.dylib" ]; then echo "$d/lib$lib.dylib"; else echo "$d/lib$lib.so"; fi
}

# Generated sources: ios/Generated/{dragent_e2ee_ffi.swift, include/*}
# and android/src/main/java/uniffi/dragent_e2ee_ffi/dragent_e2ee_ffi.kt.
generate_into() {
  local dest="$1" dylib
  dylib="$(host_dylib)"
  local tmp; tmp="$(mktemp -d)"
  cargo run --manifest-path "$manifest" --locked --features bindgen --bin uniffi-bindgen -q -- \
    generate --library "$dylib" --language swift --out-dir "$tmp/swift" --no-format >&2
  cargo run --manifest-path "$manifest" --locked --features bindgen --bin uniffi-bindgen -q -- \
    generate --library "$dylib" --language kotlin --out-dir "$tmp/kotlin" --no-format >&2
  mkdir -p "$dest/ios/Generated/include" "$dest/android/src/main/java/uniffi/$lib"
  cp "$tmp/swift/$lib.swift" "$dest/ios/Generated/$lib.swift"
  cp "$tmp/swift/${lib}FFI.h" "$dest/ios/Generated/include/${lib}FFI.h"
  # A framework-style module map so `import dragent_e2ee_ffiFFI` resolves
  # from the xcframework's Headers in the app module and in the NSE.
  printf 'module %sFFI {\n    header "%sFFI.h"\n    export *\n}\n' "$lib" "$lib" \
    > "$dest/ios/Generated/include/module.modulemap"
  cp "$tmp/kotlin/uniffi/$lib/$lib.kt" "$dest/android/src/main/java/uniffi/$lib/$lib.kt"
  rm -rf "$tmp"
}

# The committed bindings match the shipped library. Vector builds export one
# more function, so they get their own bindings under build/vectors/ (never
# committed, never linked into the app).
bindings_root() {
  if [ "$vectors" = 1 ]; then echo "$out_root/bindings"; else echo "$module"; fi
}

bindings() {
  local dest; dest="$(bindings_root)"
  generate_into "$dest"
  echo "bindings: regenerated in $dest"
}

check_bindings() {
  local tmp; tmp="$(mktemp -d)"
  generate_into "$tmp"
  local fail=0
  for f in "ios/Generated/$lib.swift" "ios/Generated/include/${lib}FFI.h" \
           "ios/Generated/include/module.modulemap" "android/src/main/java/uniffi/$lib/$lib.kt"; do
    if ! cmp -s "$tmp/$f" "$module/$f"; then
      echo "check-bindings: $f is stale; run scripts/build-e2ee-rust.sh bindings" >&2
      fail=1
    fi
  done
  rm -rf "$tmp"
  [ "$fail" = 0 ] || exit 1
  echo "check-bindings: ok"
}

# The static library must not carry code from crates that only uniffi's
# bindgen pipeline needs (tempfile, a second getrandom): LTO drops it, and
# this proves it did.
check_binary() {
  local a="$1"
  if nm -g "$a" 2>/dev/null | grep -Eq 'tempfile|getrandom.*v0_4|10getrandom.*imp.*0_4'; then
    echo "build-e2ee-rust: $a links bindgen-only code (tempfile/getrandom 0.4)" >&2
    exit 1
  fi
}

ios() {
  [ "$(uname -s)" = Darwin ] || { echo "ios: needs macOS" >&2; exit 2; }
  need xcodebuild "install Xcode"
  local targets=(aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios)
  rustup target add "${targets[@]}" >/dev/null
  for t in "${targets[@]}"; do
    IPHONEOS_DEPLOYMENT_TARGET="$ios_min" cargo build "${cargo_flags[@]}" --target "$t"
  done
  # The app's xcframework sits inside the pod (ios/Frameworks, gitignored);
  # the vectors build stays under build/vectors so it is never linked in.
  local tdir="$CARGO_TARGET_DIR" out="$module/ios/Frameworks" stage
  [ "$vectors" = 1 ] && out="$out_root/ios"
  stage="$(mktemp -d)"
  # Headers live in a subfolder named after the module. The app (through the
  # DragentE2EE pod) and the notification service extension each link this
  # xcframework, so Xcode copies its headers into two search paths of the same
  # build. A top-level module.modulemap in both is loaded eagerly by clang and
  # fails with "redefinition of module"; a <Module>/module.modulemap is only
  # read when that module is imported, and the first copy found wins.
  local hdrs="$stage/headers/${lib}FFI"
  mkdir -p "$stage/device" "$stage/sim" "$hdrs"
  cp "$tdir/aarch64-apple-ios/$profile/lib$lib.a" "$stage/device/lib$lib.a"
  lipo -create "$tdir/aarch64-apple-ios-sim/$profile/lib$lib.a" "$tdir/x86_64-apple-ios/$profile/lib$lib.a" \
    -output "$stage/sim/lib$lib.a"
  check_binary "$stage/device/lib$lib.a"
  [ "$vectors" = 1 ] && generate_into "$(bindings_root)"
  cp "$(bindings_root)/ios/Generated/include/"* "$hdrs/"
  rm -rf "$out/DragentE2EEFFI.xcframework"
  mkdir -p "$out"
  xcodebuild -create-xcframework \
    -library "$stage/device/lib$lib.a" -headers "$stage/headers" \
    -library "$stage/sim/lib$lib.a" -headers "$stage/headers" \
    -output "$out/DragentE2EEFFI.xcframework" >/dev/null
  rm -rf "$stage"
  echo "ios: $out/DragentE2EEFFI.xcframework"
}

android() {
  local ndk="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}"
  [ -n "$ndk" ] && [ -d "$ndk" ] || { echo "android: set ANDROID_NDK_HOME to an installed NDK" >&2; exit 2; }
  cargo ndk --version >/dev/null 2>&1 || { echo "android: install cargo-ndk (cargo install cargo-ndk --version 4.1.2 --locked)" >&2; exit 2; }
  rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android i686-linux-android >/dev/null
  local out="$module/android/src/main/jniLibs"
  [ "$vectors" = 1 ] && out="$out_root/android/jniLibs"
  mkdir -p "$out"
  # cargo-ndk reads the crate from the folder it runs in (not from
  # --manifest-path): run it there, with absolute paths in and out.
  local crate abs_out
  crate="$(cd "$(dirname "$manifest")" && pwd)"
  abs_out="$(cd "$out" && pwd)"
  (cd "$crate" && ANDROID_NDK_HOME="$ndk" cargo ndk --platform "$android_api" \
    -t arm64-v8a -t armeabi-v7a -t x86_64 -t x86 -o "$abs_out" \
    build "${cargo_flags[@]/#$manifest/$crate/Cargo.toml}")
  echo "android: $out"
}

# EAS build images have no Rust. Install the toolchain with rustup (only on
# EAS, never on a developer machine), then build the platform being built.
eas() {
  [ -n "${EAS_BUILD:-}" ] || { echo "eas: only runs inside an EAS build" >&2; exit 2; }
  if ! command -v cargo >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
  fi
  check_pins
  check_bindings
  case "${EAS_BUILD_PLATFORM:-}" in
    ios) ios ;;
    android)
      cargo ndk --version >/dev/null 2>&1 || cargo install cargo-ndk --version 4.1.2 --locked
      android
      ;;
    *) echo "eas: unknown EAS_BUILD_PLATFORM '${EAS_BUILD_PLATFORM:-}'" >&2; exit 2 ;;
  esac
}

case "$cmd" in
  pins) check_pins ;;
  eas) eas ;;
  bindings) check_pins; bindings ;;
  check-bindings) check_bindings ;;
  ios) check_pins; ios ;;
  android) check_pins; android ;;
  all)
    check_pins
    check_bindings
    [ "$(uname -s)" = Darwin ] && ios
    if [ -n "${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}" ]; then android; else echo "android: skipped (no ANDROID_NDK_HOME)"; fi
    ;;
  *) echo "usage: build-e2ee-rust.sh pins|bindings|check-bindings|ios|android|all|eas [--vectors] [--debug]" >&2; exit 2 ;;
esac
