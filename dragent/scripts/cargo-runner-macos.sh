#!/bin/sh
# Cargo runner for macOS dev builds (`cargo run` / `cargo test`).
#
# Two jobs, both required for TCC-gated APIs (speech recognition, microphone)
# to work in unbundled dev binaries:
#
# 1. Re-sign so the embedded __TEXT,__info_plist section gets BOUND into the
#    code signature. The linker's fast ad-hoc signature leaves
#    "Info.plist=not bound" (see `codesign -d -vvv`) and TCC reads usage
#    descriptions through the signature.
#
# 2. Exec through tcc-disclaim so the binary is its OWN "responsible process".
#    Otherwise TCC attributes the permission check to the launching terminal
#    (Cursor/Terminal/VS Code) and consults THAT app's Info.plist — Cursor has
#    no NSSpeechRecognitionUsageDescription, so speech requests SIGABRT the
#    process regardless of our own plist.

script_dir="$(cd "$(dirname "$0")" && pwd)"
helper="${TMPDIR:-/tmp}/dragent-tcc-disclaim"

codesign -f -s - "$1" 2>/dev/null || true

# (Re)build the disclaim helper when missing or its source changed.
if [ ! -x "$helper" ] || [ "$script_dir/tcc-disclaim.c" -nt "$helper" ]; then
    cc -O2 -o "$helper" "$script_dir/tcc-disclaim.c" 2>/dev/null || {
        # No compiler? Run directly — voice will only work from terminals
        # whose own Info.plist carries the speech usage key.
        exec "$@"
    }
fi

exec "$helper" "$@"
