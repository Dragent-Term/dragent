#!/usr/bin/env bash
# Before a push: scan the commits about to leave this machine for secrets.
set -euo pipefail
command -v gitleaks >/dev/null 2>&1 || { echo "pre-push: gitleaks isn't installed; skipping (CI still scans)." >&2; exit 0; }
zero=0000000000000000000000000000000000000000
while read -r _local local_sha _remote remote_sha; do
  [ "$local_sha" = "$zero" ] && continue
  if [ "$remote_sha" = "$zero" ]; then range="$local_sha --not --remotes"; else range="$remote_sha..$local_sha"; fi
  if ! gitleaks git --log-opts="$range" --redact --no-banner --config .github/gitleaks.toml >/dev/null 2>&1; then
    echo "pre-push: gitleaks found a possible secret in the commits being pushed." >&2
    echo "Run: gitleaks git --log-opts='$range' --redact -v" >&2
    exit 1
  fi
done
