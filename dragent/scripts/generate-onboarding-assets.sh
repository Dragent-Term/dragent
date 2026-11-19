#!/usr/bin/env bash
# Generate placeholder onboarding PNGs when real screenshots are not checked in.
#
# Outputs under app/assets/onboarding/:
#   feature-workspace.png
#   theme-workspace.png
#
# Replace these with real UI screenshots any time — paths stay the same.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cargo run --quiet --manifest-path "$ROOT/scripts/onboarding-asset-gen/Cargo.toml"
echo "Onboarding PNGs ready under app/assets/onboarding/"
