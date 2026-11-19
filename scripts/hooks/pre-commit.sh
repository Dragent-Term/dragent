#!/usr/bin/env bash
# Local gate before a commit: no credentials, formatted Go and Rust.
# CI runs the full checks; this catches the common mistakes before they're
# in history (where a secret stays even after it's deleted).
set -euo pipefail

staged=$(git diff --cached --name-only --diff-filter=ACMR)
[ -z "$staged" ] && exit 0
fail=0

# 1. Files that are credentials by their nature.
blocked=$(printf '%s\n' "$staged" | grep -Ei '(^|/)\.env(\.[a-z]+)?$|\.pem$|\.p8$|\.p12$|\.pfx$|\.key$|\.jks$|\.keystore$|(^|/)id_(rsa|ed25519|ecdsa)$|service[-_]?account.*\.json$' | grep -Ev '\.example$|\.sample$' || true)
if [ -n "$blocked" ]; then
  echo "pre-commit: these look like credentials and can't be committed:" >&2
  printf '  %s\n' $blocked >&2
  fail=1
fi

# 2. Secrets inside the change (gitleaks, when installed).
if command -v gitleaks >/dev/null 2>&1; then
  if ! gitleaks git --staged --redact --no-banner --config .github/gitleaks.toml >/dev/null 2>&1; then
    echo "pre-commit: gitleaks found a possible secret. Run: gitleaks git --staged --redact -v" >&2
    fail=1
  fi
else
  echo "pre-commit: gitleaks isn't installed; skipping the secret scan (CI still runs it)." >&2
fi

# 3. Very large files (build output, recordings) don't belong in the repo.
for f in $staged; do
  if [ -f "$f" ] && [ "$(wc -c < "$f")" -gt 5242880 ]; then
    echo "pre-commit: $f is over 5 MB." >&2
    fail=1
  fi
done

# 4. Go formatting.
go_files=$(printf '%s\n' "$staged" | grep -E '\.go$' || true)
if [ -n "$go_files" ] && command -v gofmt >/dev/null 2>&1; then
  unformatted=$(gofmt -l $go_files || true)
  if [ -n "$unformatted" ]; then
    echo "pre-commit: run gofmt -w on:" >&2
    printf '  %s\n' $unformatted >&2
    fail=1
  fi
fi

# 5. Rust formatting (only the files being committed).
rs_files=$(printf '%s\n' "$staged" | grep -E '^dragent/.*\.rs$' || true)
if [ -n "$rs_files" ] && command -v rustfmt >/dev/null 2>&1; then
  if ! rustfmt --check --edition 2021 $rs_files >/dev/null 2>&1; then
    echo "pre-commit: run rustfmt --edition 2021 on the staged Rust files." >&2
    fail=1
  fi
fi

exit $fail
