#!/usr/bin/env bash
# Package Dragent for macOS (.app bundle + optional .dmg).
#
# Examples:
#   ./scripts/package-macos.sh
#   ./scripts/package-macos.sh --arch universal --profile rlto
#   ./scripts/package-macos.sh --sign "Developer ID Application: Your Name (TEAMID)"
#
# Environment:
#   CODESIGN_IDENTITY   Override signing identity (default: ad-hoc "-")

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

APP_NAME="Dragent"
BUNDLE_ID="com.dragent.app"
BINARY_NAME="dragent"
INFO_PLIST_TEMPLATE="$ROOT/app/assets/resources/mac/Dragent-Info.plist"
ENTITLEMENTS="$ROOT/app/assets/resources/mac/Dragent.entitlements"
ICON_PATH="$ROOT/app/assets/resources/mac/Dragent.icns"
DMG_BG_1X="$ROOT/app/assets/resources/mac/dmg-background.png"
DMG_BG_2X="$ROOT/app/assets/resources/mac/dmg-background@2x.png"
# Icon slot centers — MUST match APP_XY / APPS_XY in scripts/macos/gen-dmg-background.py.
DMG_APP_POS="170, 205"
DMG_APPS_POS="490, 205"
DMG_ICON_SIZE=112

ARCH="auto"
PROFILE="release"
OUTPUT_DIR="$ROOT/dist"
CREATE_DMG=1
DO_SIGN=1
DO_BUILD=1
CODESIGN_IDENTITY="${CODESIGN_IDENTITY:--}"

usage() {
  cat <<'EOF'
Usage: ./scripts/package-macos.sh [options]

Options:
  --arch arm64|x86_64|universal|auto
                                Build target (default: auto)
                                auto = native arch on this Mac (arm64 on Apple Silicon)
  --profile release|rlto        Cargo profile (default: release)
  --output-dir PATH             Output directory (default: dist/)
  --no-build                    Skip cargo build; reuse binaries in target/
  --no-dmg                      Skip .dmg creation
  --no-sign                     Skip codesign
  --sign IDENTITY               Sign with this identity (default: ad-hoc "-")
  -h, --help                    Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --arch)
      ARCH="${2:?--arch requires a value}"
      shift 2
      ;;
    --profile)
      PROFILE="${2:?--profile requires a value}"
      shift 2
      ;;
    --output-dir)
      OUTPUT_DIR="${2:?--output-dir requires a value}"
      shift 2
      ;;
    --no-build)
      DO_BUILD=0
      shift
      ;;
    --no-dmg)
      CREATE_DMG=0
      shift
      ;;
    --no-sign)
      DO_SIGN=0
      shift
      ;;
    --sign)
      CODESIGN_IDENTITY="${2:?--sign requires an identity}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

case "$ARCH" in
  auto|arm64|x86_64|universal) ;;
  *)
    echo "Invalid --arch: $ARCH (expected auto, arm64, x86_64, or universal)" >&2
    exit 1
    ;;
esac

case "$PROFILE" in
  release|rlto) ;;
  *)
    echo "Invalid --profile: $PROFILE (expected release or rlto)" >&2
    exit 1
    ;;
esac

if [[ ! -f "$INFO_PLIST_TEMPLATE" ]]; then
  echo "Missing Info.plist template: $INFO_PLIST_TEMPLATE" >&2
  exit 1
fi

if [[ ! -f "$ENTITLEMENTS" ]]; then
  echo "Missing entitlements: $ENTITLEMENTS" >&2
  exit 1
fi

if [[ ! -f "$ICON_PATH" ]]; then
  echo "App icon missing at $ICON_PATH — generating from source PNG ..."
  "$ROOT/scripts/generate-app-icons.sh"
fi

VERSION="$(grep -E '^version = ' app/Cargo.toml | head -1 | sed -E 's/^version = "([^"]+)".*/\1/')"
if [[ -z "$VERSION" ]]; then
  echo "Could not read version from app/Cargo.toml" >&2
  exit 1
fi

HOST_ARCH="$(rustc -vV | awk '/^host: / { print $2 }')"
case "$HOST_ARCH" in
  aarch64-apple-darwin) NATIVE_ARCH="arm64" ;;
  x86_64-apple-darwin) NATIVE_ARCH="x86_64" ;;
  *)
    echo "Unsupported macOS host: $HOST_ARCH" >&2
    exit 1
    ;;
esac

if [[ "$ARCH" == "auto" ]]; then
  ARCH="$NATIVE_ARCH"
fi

ensure_target() {
  local target="$1"
  if ! rustup target list --installed | grep -qx "$target"; then
    echo "Installing Rust target: $target"
    rustup target add "$target"
  fi
}

build_binary() {
  local target="${1:-}"
  local out_path

  local cargo_args=(build -p dragent --profile "$PROFILE")
  if [[ -n "$target" ]]; then
    cargo_args+=(--target "$target")
    out_path="target/$target/$PROFILE/$BINARY_NAME"
  else
    out_path="target/$PROFILE/$BINARY_NAME"
  fi

  echo "Building $out_path ..." >&2
  cargo "${cargo_args[@]}" >&2

  if [[ ! -f "$out_path" ]]; then
    echo "Build succeeded but binary not found: $out_path" >&2
    exit 1
  fi

  printf '%s\n' "$out_path"
}

STAGING_BINARY="$OUTPUT_DIR/.staging/$BINARY_NAME"
mkdir -p "$OUTPUT_DIR/.staging"

if [[ "$DO_BUILD" -eq 1 ]]; then
  case "$ARCH" in
    arm64)
      ensure_target "aarch64-apple-darwin"
      built="$(build_binary "aarch64-apple-darwin")"
      cp "$built" "$STAGING_BINARY"
      ;;
    x86_64)
      ensure_target "x86_64-apple-darwin"
      built="$(build_binary "x86_64-apple-darwin")"
      cp "$built" "$STAGING_BINARY"
      ;;
    universal)
      ensure_target "aarch64-apple-darwin"
      ensure_target "x86_64-apple-darwin"
      arm_bin="$(build_binary "aarch64-apple-darwin")"
      x64_bin="$(build_binary "x86_64-apple-darwin")"
      echo "Creating universal binary ..." >&2
      lipo -create "$arm_bin" "$x64_bin" -output "$STAGING_BINARY"
      ;;
  esac
else
  case "$ARCH" in
    arm64)
      candidate="target/aarch64-apple-darwin/$PROFILE/$BINARY_NAME"
      ;;
    x86_64)
      candidate="target/x86_64-apple-darwin/$PROFILE/$BINARY_NAME"
      ;;
    universal)
      candidate=""
      ;;
  esac

  if [[ "$ARCH" == "universal" ]]; then
    arm_candidate="target/aarch64-apple-darwin/$PROFILE/$BINARY_NAME"
    x64_candidate="target/x86_64-apple-darwin/$PROFILE/$BINARY_NAME"
    if [[ -f "$arm_candidate" && -f "$x64_candidate" ]]; then
      lipo -create "$arm_candidate" "$x64_candidate" -output "$STAGING_BINARY"
    else
      echo "Missing binaries for universal package. Build first or omit --no-build." >&2
      exit 1
    fi
  elif [[ -f "$candidate" ]]; then
    cp "$candidate" "$STAGING_BINARY"
  elif [[ -f "target/$PROFILE/$BINARY_NAME" && "$ARCH" == "$NATIVE_ARCH" ]]; then
    cp "target/$PROFILE/$BINARY_NAME" "$STAGING_BINARY"
  else
    echo "No existing binary found for --arch $ARCH (tried $candidate)" >&2
    exit 1
  fi
fi

chmod +x "$STAGING_BINARY"

# Ship the repo's skills/ so the agent has its built-in skills out of the box
# (skill discovery reads Contents/Resources/skills as the lowest-priority
# source; user project/global skills of the same name override them).
bundle_skills() {
  if [[ -d "$ROOT/skills" ]]; then
    mkdir -p "$RESOURCES_DIR/skills"
    cp -R "$ROOT/skills/." "$RESOURCES_DIR/skills/"
    local count
    count="$(find "$RESOURCES_DIR/skills" -name SKILL.md | wc -l | tr -d ' ')"
    echo "Bundled $count built-in skills"
  else
    echo "No skills/ directory found — skipping bundled skills." >&2
  fi
}

echo "Binary architecture:"
file "$STAGING_BINARY"
if command -v lipo >/dev/null 2>&1; then
  lipo -info "$STAGING_BINARY" || true
fi

APP_DIR="$OUTPUT_DIR/${APP_NAME}.app"
MACOS_DIR="$APP_DIR/Contents/MacOS"
RESOURCES_DIR="$APP_DIR/Contents/Resources"

rm -rf "$APP_DIR"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR"

cp "$STAGING_BINARY" "$MACOS_DIR/$BINARY_NAME"
# Apple's bundle versions are dot-separated numbers only: a release
# candidate (0.2.9-rc.1) is 0.2.9 there. The file names keep the full version.
PLIST_VERSION="${VERSION%%-*}"
PLIST_VERSION="${PLIST_VERSION%%+*}"
sed "s/__VERSION__/$PLIST_VERSION/g" "$INFO_PLIST_TEMPLATE" > "$APP_DIR/Contents/Info.plist"

# TCC reads these keys from the final app bundle, not from the source template
# or the executable's embedded plist. Fail before signing if either key is
# missing, otherwise the released app can start voice input without ever
# presenting the system prompt.
for privacy_key in NSMicrophoneUsageDescription NSSpeechRecognitionUsageDescription; do
  if ! /usr/libexec/PlistBuddy -c "Print :$privacy_key" "$APP_DIR/Contents/Info.plist" >/dev/null 2>&1; then
    echo "::error::Final app Info.plist is missing $privacy_key — macOS will not prompt for voice access." >&2
    exit 1
  fi
done
echo "Verified macOS voice privacy usage descriptions."

if [[ -f "$ICON_PATH" ]]; then
  cp "$ICON_PATH" "$RESOURCES_DIR/AppIcon.icns"
  /usr/libexec/PlistBuddy -c "Add :CFBundleIconFile string AppIcon.icns" "$APP_DIR/Contents/Info.plist" 2>/dev/null \
    || /usr/libexec/PlistBuddy -c "Set :CFBundleIconFile AppIcon.icns" "$APP_DIR/Contents/Info.plist"
  echo "Bundled app icon from $ICON_PATH"
else
  echo "No .icns icon at $ICON_PATH (skipping; add Dragent.icns to bundle a Dock icon)"
fi

bundle_skills

# Ship the third-party license notices with the app so every recipient receives
# a copy of each bundled open-source license. Required by the AGPL-3.0-only
# (sections 4-6) and by the MIT / SIL OFL / ISC / NCSA notice requirements.
for legal in THIRD_PARTY_NOTICES.md NOTICE LICENSE; do
  if [[ -f "$ROOT/$legal" ]]; then
    cp "$ROOT/$legal" "$RESOURCES_DIR/$legal"
  fi
done
echo "Bundled legal notices (THIRD_PARTY_NOTICES.md, NOTICE, LICENSE)"

if [[ "$DO_SIGN" -eq 1 ]]; then
  echo "Signing ${APP_NAME}.app with identity: $CODESIGN_IDENTITY"
  # --entitlements is NOT optional: under the hardened runtime the microphone is
  # denied silently without com.apple.security.device.audio-input (no prompt, no
  # Privacy pane entry, silent audio buffers). See Dragent.entitlements.
  SIGN_FLAGS=(--force --deep --sign "$CODESIGN_IDENTITY" --entitlements "$ENTITLEMENTS")
  # Real (non-ad-hoc) identities get the hardened runtime + secure timestamp —
  # both are REQUIRED for notarization to pass.
  if [[ "$CODESIGN_IDENTITY" != "-" ]]; then
    SIGN_FLAGS+=(--options runtime --timestamp)
  fi
  codesign "${SIGN_FLAGS[@]}" "$APP_DIR"
  codesign --verify --deep --strict "$APP_DIR"

  # Guard: codesign PRINTS "Failed to parse entitlements" and still EXITS 0, so a
  # malformed plist silently ships a bundle with none of these. Every entitlement
  # below is load-bearing, and each one fails silently at runtime when absent —
  # a voice button that records nothing, an indexer that chunks nothing. Fail the
  # build here instead of shipping a feature that quietly does nothing.
  signed_entitlements="$(codesign -d --entitlements - --xml "$APP_DIR" 2>/dev/null || true)"
  for ent in \
    com.apple.security.device.audio-input \
    com.apple.security.cs.disable-library-validation
  do
    if ! grep -q "$ent" <<<"$signed_entitlements"; then
      echo "::error::Signed bundle is missing $ent — the feature relying on it would be silently dead at runtime." >&2
      exit 1
    fi
  done
  echo "  Entitlements verified: microphone + ONNX library loading present."
fi

# Build a styled "drag to Applications" DMG: app on the left, /Applications alias
# on the right, our branded background with an arrow between them. Needs a GUI
# Finder session for the AppleScript layout; returns non-zero if that's not
# available (headless), so the caller can fall back to a plain DMG.
make_styled_dmg() {
  local app_dir="$1" dmg_path="$2" volname="$3"
  [[ -f "$DMG_BG_1X" ]] || return 1

  local stage rw bg
  stage="$(mktemp -d)"
  rw="$(mktemp -u).dmg"

  cp -R "$app_dir" "$stage/"
  ln -s /Applications "$stage/Applications"
  mkdir -p "$stage/.background"
  # Retina-crisp background via a multi-rep TIFF when possible; else the 1x PNG.
  if command -v tiffutil >/dev/null 2>&1 && [[ -f "$DMG_BG_2X" ]] \
     && tiffutil -cathidpicheck "$DMG_BG_1X" "$DMG_BG_2X" -out "$stage/.background/background.tiff" >/dev/null 2>&1; then
    bg="background.tiff"
  else
    cp "$DMG_BG_1X" "$stage/.background/background.png"; bg="background.png"
  fi

  hdiutil create -volname "$volname" -srcfolder "$stage" -ov -format UDRW -fs HFS+ "$rw" >/dev/null || { rm -rf "$stage"; return 1; }
  hdiutil attach -readwrite -noverify -noautoopen "$rw" >/dev/null || { rm -f "$rw"; rm -rf "$stage"; return 1; }

  osascript <<OSA >/dev/null 2>&1 || true
tell application "Finder"
  tell disk "$volname"
    open
    set current view of container window to icon view
    set toolbar visible of container window to false
    set statusbar visible of container window to false
    set the bounds of container window to {200, 120, 860, 520}
    set opts to the icon view options of container window
    set arrangement of opts to not arranged
    set icon size of opts to $DMG_ICON_SIZE
    set background picture of opts to file ".background:$bg"
    set position of item "${APP_NAME}.app" of container window to {$DMG_APP_POS}
    set position of item "Applications" of container window to {$DMG_APPS_POS}
    update without registering applications
    delay 1
    close
  end tell
end tell
OSA

  sync
  hdiutil detach "/Volumes/$volname" >/dev/null 2>&1 || hdiutil detach "/Volumes/$volname" -force >/dev/null 2>&1 || true
  rm -f "$dmg_path"
  hdiutil convert "$rw" -format UDZO -imagekey zlib-level=9 -o "$dmg_path" >/dev/null || { rm -f "$rw"; rm -rf "$stage"; return 1; }
  rm -f "$rw"; rm -rf "$stage"
  return 0
}

DMG_PATH="$OUTPUT_DIR/${APP_NAME}-${VERSION}-macos-${ARCH}.dmg"
if [[ "$CREATE_DMG" -eq 1 ]]; then
  rm -f "$DMG_PATH"
  echo "Creating disk image: $DMG_PATH"
  if make_styled_dmg "$APP_DIR" "$DMG_PATH" "$APP_NAME"; then
    echo "  Styled drag-to-Applications layout applied."
  else
    echo "  Styled layout unavailable (headless?) — creating a plain DMG."
    hdiutil create -volname "$APP_NAME" -srcfolder "$APP_DIR" -ov -format UDZO "$DMG_PATH" >/dev/null
  fi

  if [[ "$DO_SIGN" -eq 1 && "$CODESIGN_IDENTITY" != "-" ]]; then
    echo "Signing DMG ..."
    codesign --force --sign "$CODESIGN_IDENTITY" "$DMG_PATH"
  fi
fi

rm -rf "$OUTPUT_DIR/.staging"

echo ""
echo "Packaging complete."
echo "  Version:     $VERSION"
echo "  Arch:        $ARCH"
echo "  App bundle:  $APP_DIR"
if [[ "$CREATE_DMG" -eq 1 ]]; then
  echo "  Disk image:  $DMG_PATH"
fi
echo ""
echo "Test locally:"
echo "  open \"$APP_DIR\""
echo ""
if [[ "$CODESIGN_IDENTITY" == "-" ]]; then
  echo "For public release, re-run with a Developer ID certificate:"
  echo "  ./scripts/package-macos.sh --sign \"Developer ID Application: Your Name (TEAMID)\""
  echo "Then notarize the DMG with xcrun notarytool and staple it."
fi
