#!/usr/bin/env bash
# Package Dragent for Linux (.tar.gz portable layout; basis for .deb / AppImage).
#
# Layout:
#   dist/linux-dragent/
#     dragent
#
# Examples:
#   ./scripts/package-linux.sh
#   ./scripts/package-linux.sh --profile release

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

BINARY_NAME="dragent"
PROFILE="release"
OUTPUT_DIR="$ROOT/dist"
DO_BUILD=1

while [[ $# -gt 0 ]]; do
  case "$1" in
    --profile)
      PROFILE="${2:?--profile requires a value}"
      shift 2
      ;;
    --no-build)
      DO_BUILD=0
      shift
      ;;
    -h|--help)
      echo "Usage: ./scripts/package-linux.sh [--profile release|rlto] [--no-build]"
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      exit 1
      ;;
  esac
done

VERSION="$(grep -E '^version = ' app/Cargo.toml | head -1 | sed -E 's/^version = "([^"]+)".*/\1/')"

if [[ "$DO_BUILD" -eq 1 ]]; then
  echo "Building dragent ($PROFILE) ..."
  cargo build -p dragent --profile "$PROFILE"
fi

BUILT="target/$PROFILE/$BINARY_NAME"
if [[ ! -f "$BUILT" ]]; then
  echo "Binary not found: $BUILT" >&2
  exit 1
fi

STAGE="$OUTPUT_DIR/linux-dragent"
LINUX_ICONS="$ROOT/app/assets/resources/linux/hicolor"
LINUX_DESKTOP="$ROOT/app/assets/resources/linux/dragent.desktop"
rm -rf "$STAGE"
mkdir -p "$STAGE"

cp "$BUILT" "$STAGE/$BINARY_NAME"
chmod +x "$STAGE/$BINARY_NAME"

# Ship the third-party license notices with the app so every recipient receives
# a copy of each bundled open-source license. Required by the AGPL-3.0-only
# (sections 4-6) and by the MIT / SIL OFL / ISC / NCSA notice requirements.
for legal in THIRD_PARTY_NOTICES.md NOTICE LICENSE; do
  [[ -f "$ROOT/$legal" ]] && cp "$ROOT/$legal" "$STAGE/$legal"
done

if [[ ! -d "$LINUX_ICONS/256x256/apps" ]]; then
  echo "Linux launcher icons missing — generating from source PNG ..."
  "$ROOT/scripts/generate-app-icons.sh"
fi

if [[ -d "$LINUX_ICONS" ]]; then
  mkdir -p "$STAGE/share/icons" "$STAGE/share/applications"
  cp -R "$LINUX_ICONS" "$STAGE/share/icons/"
  if [[ -f "$LINUX_DESKTOP" ]]; then
    DESKTOP_DEST="$STAGE/share/applications/dragent.desktop"
    cp "$LINUX_DESKTOP" "$DESKTOP_DEST"
    # Portable layout: point Exec at the bundled binary and pass callback URLs (%u).
    sed -i.bak "s|^Exec=.*|Exec=$STAGE/$BINARY_NAME %u|" "$DESKTOP_DEST"
    rm -f "${DESKTOP_DEST}.bak"
  fi
  echo "Bundled .desktop entry and hicolor icons"
fi

ARCHIVE="$OUTPUT_DIR/Dragent-${VERSION}-linux-x86_64.tar.gz"
tar -czf "$ARCHIVE" -C "$OUTPUT_DIR" "linux-dragent"

echo ""
echo "Linux packaging complete."
echo "  Staging dir: $STAGE"
echo "  Archive:     $ARCHIVE"
echo ""
echo "For .deb, install to:"
echo "  /usr/bin/dragent"
echo "  /usr/share/applications/dragent.desktop  (MimeType=x-scheme-handler/dragent)"
echo ""
echo "After install, register the auth URL handler (once per user):"
echo "  xdg-mime default dragent.desktop x-scheme-handler/dragent"
