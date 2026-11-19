#!/usr/bin/env bash
# Build and launch an unmistakable development app, isolated from installed Dragent.
set -euo pipefail
cd "$(dirname "$0")/.."
if [ "$(uname -s)" != Darwin ]; then
  echo 'This app-bundle launcher requires macOS.' >&2
  exit 1
fi
export CARGO_INCREMENTAL=0
cargo build --offline -p dragent --bin dragent -p dragent_host --bin dragent-host
dev_root="${DRAGENT_DEV_ROOT:-$PWD/target/remote-dev}"
mkdir -p "$dev_root"
dev_root=$(cd "$dev_root" && pwd)
export DRAGENT_HOME="${DRAGENT_HOME:-$dev_root/data}"
mkdir -p "$DRAGENT_HOME"
mkdir -p "$dev_root/Dragent Dev.app/Contents/Resources"
cp "$PWD/target/debug/dragent-host" "$dev_root/Dragent Dev.app/Contents/Resources/dragent-host"
python3 - "$PWD" "$dev_root" <<'PY'
import hashlib, pathlib, plistlib, shutil, sys
source, output = map(pathlib.Path, sys.argv[1:])
bundle = output / 'Dragent Dev.app'
executable = bundle / 'Contents/MacOS/dragent'
executable.parent.mkdir(parents=True, exist_ok=True)
shutil.copy2(source / 'target/debug/dragent', executable)
info = plistlib.loads((source / 'app/assets/resources/mac/Dragent-Info.plist').read_bytes())
identity = hashlib.sha256(str(source).encode()).hexdigest()[:12]
info.update(CFBundleIdentifier=f'com.dragent.dev.{identity}', CFBundleName='Dragent Dev',
            CFBundleDisplayName='Dragent Dev', CFBundleVersion='0.0.0',
            CFBundleShortVersionString='0.0.0')
info.pop('CFBundleURLTypes', None)
(bundle / 'Contents/Info.plist').write_bytes(plistlib.dumps(info))
print(f'Development source: {source / "target/debug/dragent"}')
print(f'Development app: {bundle}')
print(f'Bundle identity: {info["CFBundleIdentifier"]}')
PY
codesign --force --sign - "$dev_root/Dragent Dev.app"
echo "Isolated app data: $DRAGENT_HOME"
# DRAGENT_REMOTE_BINARY, when supplied, is a path on the SSH target. Loopback
# fixtures can point it at target/debug/dragent-host; other machines need their
# own matching development binary. Do not substitute an installed release.
exec "$dev_root/Dragent Dev.app/Contents/MacOS/dragent" "$@"
