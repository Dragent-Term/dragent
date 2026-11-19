#!/usr/bin/env bash
# Generate platform app icons from the single source PNG.
#
# Source:  app/assets/dragent/icons/dragent-icon.png
# Outputs: app/assets/resources/mac/Dragent.icns
#          app/assets/resources/windows/dragent.ico
#          app/assets/resources/linux/hicolor/*/apps/dragent.png
#
# Run after updating the source icon:
#   ./scripts/generate-app-icons.sh
#
# Environment:
#   ICON_ARTWORK_SCALE   Fraction of canvas used by artwork (default: 1.0).
#                        Use 0.80 when importing new full-bleed art so the Dock
#                        icon matches macOS sizing. Only run once per new asset.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SOURCE="$ROOT/app/assets/dragent/icons/dragent-icon.png"
ICON_ARTWORK_SCALE="${ICON_ARTWORK_SCALE:-1.0}"
ICON_GEN=(cargo run --quiet --manifest-path "$ROOT/scripts/icon-gen/Cargo.toml" --)
MAC_OUT="$ROOT/app/assets/resources/mac/Dragent.icns"
WIN_OUT="$ROOT/app/assets/resources/windows/dragent.ico"
LINUX_ROOT="$ROOT/app/assets/resources/linux/hicolor"

if [[ ! -f "$SOURCE" ]]; then
  echo "Missing source icon: $SOURCE" >&2
  exit 1
fi

echo "Source: $SOURCE"
echo "Artwork scale: $ICON_ARTWORK_SCALE"

WORK_SOURCE="$SOURCE"
PADDED_SOURCE=""
if [[ "$ICON_ARTWORK_SCALE" != "1" && "$ICON_ARTWORK_SCALE" != "1.0" ]]; then
  PADDED_SOURCE="$(mktemp -t dragent-icon-padded).png"
  "${ICON_GEN[@]}" pad "$SOURCE" "$PADDED_SOURCE" --scale "$ICON_ARTWORK_SCALE"
  WORK_SOURCE="$PADDED_SOURCE"
fi

# --- macOS .icns (requires macOS sips + iconutil) ---
if [[ "$(uname -s)" == "Darwin" ]]; then
  ICONSET="$(mktemp -d)/Dragent.iconset"
  mkdir -p "$ICONSET"

  make_icon() {
    local size="$1"
    local out="$2"
    sips -z "$size" "$size" "$WORK_SOURCE" --out "$out" >/dev/null
  }

  make_icon 16   "$ICONSET/icon_16x16.png"
  make_icon 32   "$ICONSET/icon_16x16@2x.png"
  make_icon 32   "$ICONSET/icon_32x32.png"
  make_icon 64   "$ICONSET/icon_32x32@2x.png"
  make_icon 128  "$ICONSET/icon_128x128.png"
  make_icon 256  "$ICONSET/icon_128x128@2x.png"
  make_icon 256  "$ICONSET/icon_256x256.png"
  make_icon 512  "$ICONSET/icon_256x256@2x.png"
  make_icon 512  "$ICONSET/icon_512x512.png"
  make_icon 1024 "$ICONSET/icon_512x512@2x.png"

  mkdir -p "$(dirname "$MAC_OUT")"
  iconutil -c icns "$ICONSET" -o "$MAC_OUT"
  rm -rf "$(dirname "$ICONSET")"
  echo "Wrote $MAC_OUT"
else
  echo "Skipping .icns (requires macOS). Run this script on a Mac before packaging for macOS." >&2
fi

# --- Windows .ico (Rust helper; works on any host) ---
mkdir -p "$(dirname "$WIN_OUT")"
"${ICON_GEN[@]}" ico "$WORK_SOURCE" "$WIN_OUT"
echo "Wrote $WIN_OUT"

# --- Linux hicolor PNGs ---
if [[ "$(uname -s)" == "Darwin" ]]; then
  for size in 16 22 24 32 48 64 128 256 512; do
    dest="$LINUX_ROOT/${size}x${size}/apps"
    mkdir -p "$dest"
    sips -z "$size" "$size" "$WORK_SOURCE" --out "$dest/dragent.png" >/dev/null
  done
  echo "Wrote Linux hicolor icons under $LINUX_ROOT"
else
  echo "Skipping Linux PNG resize (requires macOS sips). ICO was still generated." >&2
fi

if [[ -n "$PADDED_SOURCE" ]]; then
  cp "$PADDED_SOURCE" "$SOURCE"
  echo "Updated $SOURCE with ${ICON_ARTWORK_SCALE}x artwork padding"
  rm -f "$PADDED_SOURCE"
fi

echo ""
echo "App icons generated. Commit the files under app/assets/resources/ if they changed."
