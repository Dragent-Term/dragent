#!/usr/bin/env bash
# Fetch Lucide SVG icons into app/assets/dragent/icons/.
# Usage: ./scripts/fetch-lucide-icons.sh lucide-name:local-name [...]
# Example: ./scripts/fetch-lucide-icons.sh share-2:share pencil:edit

set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/app/assets/dragent/icons"
BASE="https://raw.githubusercontent.com/lucide-icons/lucide/main/icons"

mkdir -p "$OUT"

if [[ $# -eq 0 ]]; then
  echo "Usage: $0 lucide-name:local-name [...]" >&2
  exit 1
fi

for pair in "$@"; do
  lucide="${pair%%:*}"
  local="${pair##*:}"
  curl -fsSL "$BASE/${lucide}.svg" -o "$OUT/${local}.svg"
  sed -i '' \
    -e 's/stroke="currentColor"/stroke="#ffffff"/g' \
    -e 's/fill="currentColor"/fill="#ffffff"/g' \
    "$OUT/${local}.svg"
  echo "fetched $OUT/${local}.svg (from lucide $lucide)"
done
