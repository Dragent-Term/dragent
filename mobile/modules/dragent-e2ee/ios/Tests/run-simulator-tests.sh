#!/usr/bin/env bash
# Runs the E2EE native module's tests on an iOS simulator (§12.1, §6.7, §11).
#
#   run-simulator-tests.sh vectors   debug library with the vector runners:
#                                    vectors + keychain classes + binding API
#   run-simulator-tests.sh bench     release library (what ships): keychain
#                                    classes + binding API + benchmarks
#
# Stages a SwiftPM package under modules/dragent-e2ee/build/spm-<mode> from the
# xcframework, the generated Swift binding, the shared keychain code and the
# crate's vector files, then runs `xcodebuild test` on SIMULATOR (default:
# the first available iPhone).
set -euo pipefail
mode="${1:-vectors}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
module="$here/../.."
mobile="$module/../.."
crate_vectors="$mobile/../dragent/crates/dragent_e2ee/vectors"

case "$mode" in
  vectors)
    "$mobile/scripts/build-e2ee-rust.sh" ios --vectors >&2
    xcf="$module/build/vectors/ios/DragentE2EEFFI.xcframework"
    gen="$module/build/vectors/bindings/ios/Generated"
    tests=(KeychainTests ApiTests VectorTests)
    ;;
  bench)
    "$mobile/scripts/build-e2ee-rust.sh" ios >&2
    xcf="$module/ios/Frameworks/DragentE2EEFFI.xcframework"
    gen="$module/ios/Generated"
    tests=(KeychainTests ApiTests BenchmarkTests)
    ;;
  *) echo "usage: run-simulator-tests.sh vectors|bench" >&2; exit 2 ;;
esac

pkg="$module/build/spm-$mode"
rm -rf "$pkg"
host="$pkg/Sources/DragentE2EETestHost"
mkdir -p "$pkg/Sources/DragentE2EECore" "$host/vectors" "$pkg/Frameworks"
cp -R "$xcf" "$pkg/Frameworks/"
cp "$gen/dragent_e2ee_ffi.swift" "$module/ios/Shared/DragentE2EEKeychain.swift" "$pkg/Sources/DragentE2EECore/"
for t in "${tests[@]}"; do cp "$here/$t.swift" "$host/"; done
cp "$here/TestHostMain.swift" "$host/main.swift"
cp -R "$crate_vectors/noise" "$crate_vectors/codec" "$host/vectors/"
cat > "$pkg/Package.swift" <<'SWIFT'
// swift-tools-version:5.9
import PackageDescription

let package = Package(
  name: "DragentE2EECore",
  platforms: [.iOS("16.4")],
  targets: [
    .binaryTarget(name: "DragentE2EEFFI", path: "Frameworks/DragentE2EEFFI.xcframework"),
    .target(name: "DragentE2EECore", dependencies: ["DragentE2EEFFI"], path: "Sources/DragentE2EECore"),
    .executableTarget(
      name: "DragentE2EETestHost",
      dependencies: ["DragentE2EECore"],
      path: "Sources/DragentE2EETestHost",
      resources: [.copy("vectors")],
      linkerSettings: [.linkedFramework("XCTest")]
    ),
  ]
)
SWIFT

# Entitlements of the host, the same shape as the app's: its own access group
# plus the group shared with the Notification Service Extension.
team="ABCDE12345"
cat > "$pkg/host.xcent" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>application-identifier</key><string>$team.dev.dragent.e2ee.testhost</string>
  <key>keychain-access-groups</key><array>
    <string>$team.dev.dragent.e2ee.testhost</string>
    <string>$team.dev.dragent.e2ee</string>
  </array>
</dict></plist>
PLIST

sdk="$(xcrun --sdk iphonesimulator --show-sdk-path)"
plat="$(xcode-select -p)/Platforms/iPhoneSimulator.platform/Developer"
(cd "$pkg" && swift build -c debug --triple arm64-apple-ios16.4-simulator --sdk "$sdk" \
  -Xswiftc -F -Xswiftc "$plat/Library/Frameworks" -Xswiftc -I -Xswiftc "$plat/usr/lib" \
  -Xlinker -F -Xlinker "$plat/Library/Frameworks" -Xlinker -L -Xlinker "$plat/usr/lib" \
  -Xlinker -rpath -Xlinker "$plat/Library/Frameworks" -Xlinker -rpath -Xlinker "$plat/usr/lib" \
  -Xlinker -sectcreate -Xlinker __TEXT -Xlinker __entitlements -Xlinker "$pkg/host.xcent" >&2)

bin="$pkg/.build/arm64-apple-ios-simulator/debug"
app="$pkg/DragentE2EETestHost.app"
mkdir -p "$app"
cp "$bin/DragentE2EETestHost" "$app/"
cp -R "$bin/"*.bundle "$app/" 2>/dev/null || true
cat > "$app/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>dev.dragent.e2ee.testhost</string>
  <key>CFBundleExecutable</key><string>DragentE2EETestHost</string>
  <key>CFBundleName</key><string>DragentE2EETestHost</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>LSRequiresIPhoneOS</key><true/>
  <key>MinimumOSVersion</key><string>16.4</string>
  <key>UIDeviceFamily</key><array><integer>1</integer></array>
  <key>DragentE2EESharedKeychainGroup</key><string>$(AppIdentifierPrefix)dev.dragent.e2ee</string>
  <key>DragentE2EEPrivateKeychainGroup</key><string>$(AppIdentifierPrefix)com.dragent.mobile</string>
</dict></plist>
PLIST
# Like Xcode's simulator builds: entitlements live in the __entitlements
# section (linked above); the ad-hoc signature carries none.
codesign --force --sign - --timestamp=none "$app" >&2

# A dedicated simulator (never one another session has booted).
sim="${SIMULATOR:-}"
if [ -z "$sim" ]; then
  sim="$(xcrun simctl list devices available | sed -n 's/^ *iPhone[^(]* (\([0-9A-F-]*\)) (Shutdown).*/\1/p' | head -1)"
  [ -n "$sim" ] || sim="$(xcrun simctl list devices available | sed -n 's/^ *iPhone[^(]* (\([0-9A-F-]*\)).*/\1/p' | head -1)"
fi
[ -n "$sim" ] || { echo "no available iPhone simulator" >&2; exit 2; }
was_shutdown=0
xcrun simctl list devices | grep "$sim" | grep -q "(Shutdown)" && was_shutdown=1
xcrun simctl boot "$sim" 2>/dev/null || true
xcrun simctl bootstatus "$sim" -b >/dev/null
log="$module/build/spm-$mode.log"
# simctl spawn runs the signed binary inside the simulator (launchd, no
# SpringBoard); the path must not contain spaces.
stage="$(mktemp -d)"
cp -R "$app" "$stage/"
SIMCTL_CHILD_DYLD_FRAMEWORK_PATH="$plat/Library/Frameworks" SIMCTL_CHILD_DYLD_LIBRARY_PATH="$plat/usr/lib" \
  xcrun simctl spawn "$sim" "$stage/DragentE2EETestHost.app/DragentE2EETestHost" > "$log" 2>&1 || true
rm -rf "$stage"
[ "$was_shutdown" = 1 ] && xcrun simctl shutdown "$sim" || true
echo "simulator: $(xcrun simctl list devices | grep "$sim" | sed 's/^ *//')"
grep -E "\[e2ee-(test|bench)\]|error:|failed \(|Executed" "$log" || true
grep -q "\[e2ee-test\] RESULT executed=[1-9][0-9]* failures=0" "$log"
