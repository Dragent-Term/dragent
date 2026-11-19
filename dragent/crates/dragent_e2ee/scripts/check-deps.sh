#!/usr/bin/env bash
# Dependency pin checks for dragent_e2ee (docs/design/e2ee.md §3.2).
#
#  1. `cargo tree -e features` shows no snow feature beyond the pinned set
#     (use-curve25519, use-chacha20poly1305, use-sha2) and the features those
#     imply inside snow (default-resolver and the optional deps they name).
#  2. The only getrandom linked into dragent_e2ee (any target) is 0.3.4.
#  3. No ring, blake2, aes-gcm, p256 or pqcrypto anywhere in its tree.
#  4. The desktop app never enables the test-only `vectors` feature.
#
# Usage: crates/dragent_e2ee/scripts/check-deps.sh   (from anywhere in the workspace)
# Exits non-zero with a message on any violation. Uses the committed lock file.
set -euo pipefail

CARGO="${CARGO:-cargo}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
manifest="$here/../../../Cargo.toml"
tree() { "$CARGO" tree --manifest-path "$manifest" --locked -p dragent_e2ee --target all "$@"; }

fail=0

allowed_snow='^(use-curve25519|use-chacha20poly1305|use-sha2|default-resolver|curve25519-dalek|chacha20poly1305|sha2)$'
snow_features="$(tree -e features --prefix none 2>/dev/null \
  | sed -n 's/^snow feature "\([^"]*\)".*/\1/p' | sort -u)"
if [ -z "$snow_features" ]; then
  echo "check-deps: could not read snow's features from cargo tree" >&2
  exit 2
fi
bad="$(printf '%s\n' "$snow_features" | grep -Ev "$allowed_snow" || true)"
if [ -n "$bad" ]; then
  echo "check-deps: snow has features outside the §3.2 pin:" >&2
  printf '  %s\n' $bad >&2
  fail=1
fi

for want in use-curve25519 use-chacha20poly1305 use-sha2; do
  if ! printf '%s\n' "$snow_features" | grep -qx "$want"; then
    echo "check-deps: pinned snow feature '$want' is missing" >&2
    fail=1
  fi
done

normal="$(tree -e normal --prefix none 2>/dev/null | awk '{print $1" "$2}' | sort -u)"
getrandoms="$(printf '%s\n' "$normal" | awk '$1=="getrandom"{print $2}' | sort -u)"
if [ "$getrandoms" != "v0.3.4" ]; then
  echo "check-deps: dragent_e2ee must link only getrandom v0.3.4, found:" >&2
  printf '  %s\n' $getrandoms >&2
  fail=1
fi

banned="$(printf '%s\n' "$normal" | awk '$1 ~ /^(ring|blake2|aes-gcm|aes|p256|pqcrypto-.*)$/' || true)"
if [ -n "$banned" ]; then
  echo "check-deps: banned crates in dragent_e2ee's tree:" >&2
  printf '  %s\n' "$banned" >&2
  fail=1
fi

for pin in "snow v0.10.0" "x25519-dalek v2.0.1" "chacha20poly1305 v0.10.1" \
           "curve25519-dalek v4.1.3" "hkdf v0.12.4" "sha2 v0.10.9" "zeroize v1.8.2"; do
  if ! printf '%s\n' "$normal" | grep -qx "$pin"; then
    echo "check-deps: expected pinned dependency '$pin' not found" >&2
    fail=1
  fi
done

# The `vectors` feature (fixed ephemerals, for the phone module's simulator
# tests) must never reach the desktop app: not in any workspace manifest
# that depends on dragent_e2ee, and not in the app's resolved feature set
# with its `e2ee` feature on.
ws="$(cd "$here/../../.." && pwd)"
for m in "$ws/Cargo.toml" "$ws/app/Cargo.toml"; do
  if grep -E 'dragent_e2ee' "$m" | grep -q 'vectors'; then
    echo "check-deps: $m enables dragent_e2ee's test-only 'vectors' feature" >&2
    fail=1
  fi
done
app_features="$("$CARGO" tree --manifest-path "$manifest" --locked -p dragent --features e2ee \
  -e features -i dragent_e2ee --prefix none 2>/dev/null | sed -n 's/^dragent_e2ee feature "\([^"]*\)".*/\1/p' | sort -u)"
if printf '%s\n' "$app_features" | grep -qx vectors; then
  echo "check-deps: the desktop app resolves dragent_e2ee with the test-only 'vectors' feature" >&2
  fail=1
fi

if [ "$fail" -ne 0 ]; then exit 1; fi
echo "check-deps: ok (snow features: $(echo $snow_features); getrandom: $getrandoms)"
