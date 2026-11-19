#!/usr/bin/env bash
# Copies built-in skills to ~/.dragent/skills/ so the agent can read them at runtime.
# Run once after cloning, or again to update skills to the latest version.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_SKILLS="$(cd "$SCRIPT_DIR/../skills" && pwd)"
TARGET="$HOME/.dragent/skills"

mkdir -p "$TARGET"
cp -r "$REPO_SKILLS/." "$TARGET/"

echo "Skills installed to $TARGET"
find "$TARGET" -name "SKILL.md" | wc -l | xargs -I{} echo "{} skills available"
