#!/usr/bin/env bash
# Convert Lucide SVGs for warpui Icon tinting.
# warpui uses the texture RED channel as alpha; icons must render white (#ffffff).

set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIR="$ROOT/app/assets/dragent/icons"

for f in "$DIR"/*.svg; do
  sed -i '' \
    -e 's/stroke="currentColor"/stroke="#ffffff"/g' \
    -e 's/fill="currentColor"/fill="#ffffff"/g' \
    "$f"
done

echo "normalized icons in $DIR"
