#!/usr/bin/env bash
# §12.8 device cases for the iOS Notification Service Extension (E9a), on an
# iOS simulator. Stages a SwiftPM package from the release
# DragentE2EEFFI.xcframework (what ships), the generated Swift binding, the
# shared keychain code and this plugin's extension sources, then runs the
# XCTest suite inside a host app signed with the keychain group shared with
# the extension and the App Group (an xctest agent process has neither).
#
#   bash plugins/e2ee-nse/ios/Tests/run-simulator-tests.sh   (SIMULATOR=<udid> picks one)
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
plugin="$here/../.."
mobile="$plugin/../.."
module="$mobile/modules/dragent-e2ee"

"$mobile/scripts/build-e2ee-rust.sh" ios >&2
xcf="$module/ios/Frameworks/DragentE2EEFFI.xcframework"
gen="$module/ios/Generated"

pkg="$plugin/build/spm-nse"
rm -rf "$pkg"
host="$pkg/Sources/DragentNSETestHost"
mkdir -p "$pkg/Sources/DragentE2EECore" "$host/fixtures" "$pkg/Frameworks"
cp -R "$xcf" "$pkg/Frameworks/"
cp "$gen/dragent_e2ee_ffi.swift" "$module/ios/Shared/DragentE2EEKeychain.swift" \
  "$plugin/ios/DragentPushHandler.swift" "$plugin/ios/NotificationService.swift" "$pkg/Sources/DragentE2EECore/"
cp "$here/PushHandlerTests.swift" "$host/"
cp "$module/ios/Tests/TestHostMain.swift" "$host/main.swift"
cp "$plugin/fixtures/push-fixtures.json" "$host/fixtures/"
cat > "$pkg/Package.swift" <<'SWIFT'
// swift-tools-version:5.9
import PackageDescription

let package = Package(
  name: "DragentE2EECore",
  platforms: [.iOS("16.4")],
  targets: [
    .binaryTarget(name: "DragentE2EEFFI", path: "Frameworks/DragentE2EEFFI.xcframework"),
    .target(name: "DragentE2EECore", dependencies: ["DragentE2EEFFI"], path: "Sources/DragentE2EECore",
            linkerSettings: [.linkedFramework("UserNotifications")]),
    .executableTarget(
      name: "DragentNSETestHost",
      dependencies: ["DragentE2EECore"],
      path: "Sources/DragentNSETestHost",
      resources: [.copy("fixtures")],
      linkerSettings: [.linkedFramework("XCTest")]
    ),
  ]
)
SWIFT

# The extension's entitlements, plus the host's own group: the shared
# keychain group and the App Group for the seen-id list.
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
  <key>com.apple.security.application-groups</key><array>
    <string>group.com.dragent.mobile</string>
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
app="$pkg/DragentNSETestHost.app"
mkdir -p "$app"
cp "$bin/DragentNSETestHost" "$app/"
cp -R "$bin/"*.bundle "$app/" 2>/dev/null || true
cat > "$app/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>dev.dragent.e2ee.testhost</string>
  <key>CFBundleExecutable</key><string>DragentNSETestHost</string>
  <key>CFBundleName</key><string>DragentNSETestHost</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>LSRequiresIPhoneOS</key><true/>
  <key>MinimumOSVersion</key><string>16.4</string>
  <key>UIDeviceFamily</key><array><integer>1</integer></array>
</dict></plist>
PLIST
codesign --force --sign - --timestamp=none "$app" >&2

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
log="$plugin/build/spm-nse.log"
stage="$(mktemp -d)"
cp -R "$app" "$stage/"
SIMCTL_CHILD_DYLD_FRAMEWORK_PATH="$plat/Library/Frameworks" SIMCTL_CHILD_DYLD_LIBRARY_PATH="$plat/usr/lib" \
  xcrun simctl spawn "$sim" "$stage/DragentNSETestHost.app/DragentNSETestHost" > "$log" 2>&1 || true
rm -rf "$stage"
[ "$was_shutdown" = 1 ] && xcrun simctl shutdown "$sim" || true
echo "simulator: $(xcrun simctl list devices | grep "$sim" | sed 's/^ *//')"
grep -E "\[e2ee-test\]|error:|failed \(|passed \(|Executed" "$log" || true
grep -q "\[e2ee-test\] RESULT executed=[1-9][0-9]* failures=0" "$log"
