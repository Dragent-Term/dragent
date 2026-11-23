#!/usr/bin/env bash
# Package Dragent for Linux (.tar.gz portable layout; basis for .deb / AppImage).
#
# Layout (Release; Dev uses dist/linux-dragent-dev/ instead):
#   dist/linux-dragent[ -dev]/
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
APP_NAME="Dragent"
URL_SCHEME="dragent"
DESKTOP_NAME="dragent.desktop"
STAGE_NAME="linux-dragent"
# Dev edition (every build without DRAGENT_RELEASE_EDITION=1 in the
# environment): its own display name, .desktop file and URL scheme, so its
# launcher and links never collide with the installed Dragent. The release
# workflow builds with DRAGENT_RELEASE_EDITION=1 and gets the values above,
# byte-for-byte the previous Release layout.
if [[ "${DRAGENT_RELEASE_EDITION:-}" != "1" ]]; then
  APP_NAME="Dragent Dev"
  URL_SCHEME="dragent-dev"
  DESKTOP_NAME="dragent-dev.desktop"
  STAGE_NAME="linux-dragent-dev"
fi

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

STAGE="$OUTPUT_DIR/$STAGE_NAME"
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
    DESKTOP_DEST="$STAGE/share/applications/$DESKTOP_NAME"
    cp "$LINUX_DESKTOP" "$DESKTOP_DEST"
    # Dev edition: its own display name and URL scheme (Release keeps this
    # file's values).
    if [[ "$DESKTOP_NAME" != "dragent.desktop" ]]; then
      sed -i.bak -e "s/^Name=.*/Name=$APP_NAME/" \
        -e "s|^MimeType=.*|MimeType=x-scheme-handler/$URL_SCHEME;|" \
        "$DESKTOP_DEST"
      rm -f "${DESKTOP_DEST}.bak"
    fi
    # Portable layout: point Exec at the bundled binary and pass callback URLs (%u).
    sed -i.bak "s|^Exec=.*|Exec=$STAGE/$BINARY_NAME %u|" "$DESKTOP_DEST"
    rm -f "${DESKTOP_DEST}.bak"
  fi
  echo "Bundled .desktop entry and hicolor icons"
fi

ARCHIVE="$OUTPUT_DIR/${APP_NAME}-${VERSION}-linux-x86_64.tar.gz"
tar -czf "$ARCHIVE" -C "$OUTPUT_DIR" "$STAGE_NAME"

echo ""
echo "Linux packaging complete."
echo "  Staging dir: $STAGE"
echo "  Archive:     $ARCHIVE"
echo ""
BIN_INSTALL=dragent
if [[ "$DESKTOP_NAME" != "dragent.desktop" ]]; then
  BIN_INSTALL=dragent-dev
fi
echo "For .deb, install to:"
echo "  /usr/bin/$BIN_INSTALL"
echo "  /usr/share/applications/$DESKTOP_NAME  (MimeType=x-scheme-handler/$URL_SCHEME)"
echo ""
echo "After install, register the auth URL handler (once per user):"
echo "  xdg-mime default $DESKTOP_NAME x-scheme-handler/$URL_SCHEME"
