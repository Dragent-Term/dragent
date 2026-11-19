// Expo config plugin for end-to-end encrypted push, device side
// (docs/design/e2ee.md §8, task E9a). Runs at `expo prebuild`.
//
// iOS: adds the Notification Service Extension target
//   `DragentNotificationService` (bundle id <app>.NotificationService). It
//   compiles this plugin's ios/*.swift plus E4's shared keychain code and the
//   generated uniffi binding, and links DragentE2EEFFI.xcframework (the same
//   dragent_e2ee static library as the app; no JS, no Expo pods). Its
//   entitlements list ONLY the shared keychain group
//   `$(AppIdentifierPrefix)dev.dragent.e2ee` (so the identity key, in the app's
//   private group, is unreachable) and the App Group for the seen-id list.
//   The placeholder text lives in its Info.plist (§8 step 4.5).
//
// Android: replaces expo-notifications' FirebaseMessagingService with
//   dev.dragent.e2ee.push.DragentPushService (which extends it, so token
//   refresh still reaches expo-notifications), copies the Kotlin sources and
//   the placeholder strings, and adds firebase-messaging to the app module
//   (expo-notifications only has it as an implementation dependency).
const fs = require("fs");
const path = require("path");
const {
  withAndroidManifest,
  withAppBuildGradle,
  withDangerousMod,
  withEntitlementsPlist,
  withXcodeProject,
} = require("expo/config-plugins");

const TARGET = "DragentNotificationService";
const SHARED_KEYCHAIN_GROUP = "$(AppIdentifierPrefix)dev.dragent.e2ee";
const APP_GROUP = "group.com.dragent.mobile";
const PLACEHOLDER = { title: "Dragent", body: "An agent needs you" };
const FIREBASE_MESSAGING = "com.google.firebase:firebase-messaging:25.0.1";
const DEPLOYMENT_TARGET = "16.4";
const SERVICE = "dev.dragent.e2ee.push.DragentPushService";
const EXPO_SERVICE = "expo.modules.notifications.service.ExpoFirebaseMessagingService";

const here = __dirname;
/** Agents' icons for their notifications (`agent_<key>.png`, see the handlers' maps). */
const ICON_DIR = path.join(here, "icons");
const AGENT_ICONS = fs.readdirSync(ICON_DIR).filter((name) => /^agent_[a-z]+\.png$/.test(name)).sort();
const mobile = path.resolve(here, "..", "..");
const e2eeModule = path.join(mobile, "modules", "dragent-e2ee");

/** Files compiled into the extension, by their name in the target folder. */
const SWIFT_SOURCES = {
  "NotificationService.swift": path.join(here, "ios", "NotificationService.swift"),
  "DragentPushHandler.swift": path.join(here, "ios", "DragentPushHandler.swift"),
  "DragentE2EEKeychain.swift": path.join(e2eeModule, "ios", "Shared", "DragentE2EEKeychain.swift"),
  "dragent_e2ee_ffi.swift": path.join(e2eeModule, "ios", "Generated", "dragent_e2ee_ffi.swift"),
};

function bundleId(config) {
  const id = config.ios && config.ios.bundleIdentifier;
  if (!id) throw new Error("e2ee-nse: ios.bundleIdentifier is required");
  return `${id}.NotificationService`;
}

function escapeXml(s) {
  return String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function infoPlist(config) {
  const version = escapeXml(config.version || "1.0.0");
  const build = escapeXml((config.ios && config.ios.buildNumber) || "1");
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>$(DEVELOPMENT_LANGUAGE)</string>
  <key>CFBundleDisplayName</key><string>Dragent Notifications</string>
  <key>CFBundleExecutable</key><string>$(EXECUTABLE_NAME)</string>
  <key>CFBundleIdentifier</key><string>$(PRODUCT_BUNDLE_IDENTIFIER)</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleName</key><string>$(PRODUCT_NAME)</string>
  <key>CFBundlePackageType</key><string>$(PRODUCT_BUNDLE_PACKAGE_TYPE)</string>
  <key>CFBundleShortVersionString</key><string>${version}</string>
  <key>CFBundleVersion</key><string>${build}</string>
  <key>NSExtension</key>
  <dict>
    <key>NSExtensionPointIdentifier</key><string>com.apple.usernotifications.service</string>
    <key>NSExtensionPrincipalClass</key><string>NotificationService</string>
  </dict>
  <key>DragentE2EESharedKeychainGroup</key><string>${SHARED_KEYCHAIN_GROUP}</string>
  <key>DragentPushPlaceholderTitle</key><string>${escapeXml(PLACEHOLDER.title)}</string>
  <key>DragentPushPlaceholderBody</key><string>${escapeXml(PLACEHOLDER.body)}</string>
</dict>
</plist>
`;
}

function entitlementsPlist() {
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>keychain-access-groups</key>
  <array><string>${SHARED_KEYCHAIN_GROUP}</string></array>
  <key>com.apple.security.application-groups</key>
  <array><string>${APP_GROUP}</string></array>
</dict>
</plist>
`;
}

/** Write the extension's sources, Info.plist and entitlements into ios/. */
function withExtensionFiles(config) {
  return withDangerousMod(config, [
    "ios",
    async (cfg) => {
      const dir = path.join(cfg.modRequest.platformProjectRoot, TARGET);
      fs.mkdirSync(dir, { recursive: true });
      for (const [name, src] of Object.entries(SWIFT_SOURCES)) {
        if (!fs.existsSync(src)) throw new Error(`e2ee-nse: missing ${src}`);
        fs.copyFileSync(src, path.join(dir, name));
      }
      for (const icon of AGENT_ICONS) fs.copyFileSync(path.join(ICON_DIR, icon), path.join(dir, icon));
      fs.writeFileSync(path.join(dir, "Info.plist"), infoPlist(cfg));
      fs.writeFileSync(path.join(dir, `${TARGET}.entitlements`), entitlementsPlist());
      return cfg;
    },
  ]);
}

/** Add the app-extension target to the Xcode project (idempotent). */
function withExtensionTarget(config) {
  return withXcodeProject(config, (cfg) => {
    const project = cfg.modResults;
    if (project.pbxTargetByName(TARGET)) return cfg;
    const objects = project.hash.project.objects;
    objects.PBXTargetDependency = objects.PBXTargetDependency || {};
    objects.PBXContainerItemProxy = objects.PBXContainerItemProxy || {};

    const files = [...Object.keys(SWIFT_SOURCES), ...AGENT_ICONS, "Info.plist", `${TARGET}.entitlements`];
    const group = project.addPbxGroup(files, TARGET, TARGET);
    const mainGroup = project.getFirstProject().firstProject.mainGroup;
    project.addToPbxGroup(group.uuid, mainGroup);

    const target = project.addTarget(TARGET, "app_extension", TARGET, bundleId(cfg));
    project.addBuildPhase(Object.keys(SWIFT_SOURCES), "PBXSourcesBuildPhase", "Sources", target.uuid);
    // The agents' icons, attached to their notifications.
    project.addBuildPhase(AGENT_ICONS, "PBXResourcesBuildPhase", "Resources", target.uuid);
    project.addBuildPhase([], "PBXFrameworksBuildPhase", "Frameworks", target.uuid);
    // The static-library xcframework E4 builds (gitignored; built before
    // pod install by scripts/build-e2ee-rust.sh). Xcode picks the slice and
    // its module map; no search paths needed.
    project.addFramework("../modules/dragent-e2ee/ios/Frameworks/DragentE2EEFFI.xcframework", {
      target: target.uuid,
      link: true,
      lastKnownFileType: "wrapper.xcframework",
      sourceTree: '"<group>"',
    });
    project.addFramework("UserNotifications.framework", { target: target.uuid, link: true });

    // The app target's team, so automatic signing works for both.
    const configurations = project.pbxXCBuildConfigurationSection();
    let team = null;
    for (const key of Object.keys(configurations)) {
      const s = configurations[key].buildSettings;
      if (s && s.DEVELOPMENT_TEAM && !String(s.PRODUCT_NAME || "").includes(TARGET)) team = s.DEVELOPMENT_TEAM;
    }
    for (const key of Object.keys(configurations)) {
      const s = configurations[key].buildSettings;
      if (!s || s.PRODUCT_NAME !== `"${TARGET}"`) continue;
      Object.assign(s, {
        INFOPLIST_FILE: `${TARGET}/Info.plist`,
        CODE_SIGN_ENTITLEMENTS: `${TARGET}/${TARGET}.entitlements`,
        CODE_SIGN_STYLE: "Automatic",
        IPHONEOS_DEPLOYMENT_TARGET: DEPLOYMENT_TARGET,
        SWIFT_VERSION: "5.0",
        TARGETED_DEVICE_FAMILY: '"1,2"',
        GENERATE_INFOPLIST_FILE: "NO",
        CLANG_ENABLE_MODULES: "YES",
        SWIFT_COMPILATION_MODE: "wholemodule",
        APPLICATION_EXTENSION_API_ONLY: "YES",
        OTHER_LDFLAGS: '"$(inherited) -framework Security"',
      });
      if (team) s.DEVELOPMENT_TEAM = team;
    }
    return cfg;
  });
}

/** The extension's App Group must also exist on the app id (EAS provisioning). */
function withAppGroup(config) {
  return withEntitlementsPlist(config, (cfg) => {
    const groups = cfg.modResults["com.apple.security.application-groups"] || [];
    if (!groups.includes(APP_GROUP)) groups.push(APP_GROUP);
    cfg.modResults["com.apple.security.application-groups"] = groups;
    return cfg;
  });
}

// ── Android ─────────────────────────────────────────────────────────────────

/** Swap expo-notifications' messaging service for DragentPushService. */
function applyPushService(manifest) {
  manifest.$ = manifest.$ || {};
  manifest.$["xmlns:tools"] = manifest.$["xmlns:tools"] || "http://schemas.android.com/tools";
  const app = manifest.application && manifest.application[0];
  if (!app) throw new Error("e2ee-nse: AndroidManifest has no <application>");
  app.service = (app.service || []).filter((s) => s.$["android:name"] !== SERVICE && s.$["android:name"] !== EXPO_SERVICE);
  // expo-notifications' own service would also receive MESSAGING_EVENT.
  app.service.push({ $: { "android:name": EXPO_SERVICE, "tools:node": "remove" } });
  app.service.push({
    $: { "android:name": SERVICE, "android:exported": "false" },
    "intent-filter": [{ $: { "android:priority": "1" }, action: [{ $: { "android:name": "com.google.firebase.MESSAGING_EVENT" } }] }],
  });
  return manifest;
}

/** Add firebase-messaging to the app module's dependencies (once). */
function applyFirebaseMessaging(contents) {
  if (contents.includes(FIREBASE_MESSAGING)) return contents;
  const line = `    implementation '${FIREBASE_MESSAGING}' // e2ee-nse: DragentPushService`;
  return contents.replace(/dependencies\s*\{\n/, (m) => `${m}${line}\n`);
}

function withPushService(config) {
  return withAndroidManifest(config, (cfg) => {
    applyPushService(cfg.modResults.manifest);
    return cfg;
  });
}

function withFirebaseMessaging(config) {
  return withAppBuildGradle(config, (cfg) => {
    cfg.modResults.contents = applyFirebaseMessaging(cfg.modResults.contents);
    return cfg;
  });
}

function withAndroidFiles(config) {
  return withDangerousMod(config, [
    "android",
    async (cfg) => {
      const root = cfg.modRequest.platformProjectRoot;
      const dir = path.join(root, "app", "src", "main", "java", "dev", "dragent", "e2ee", "push");
      fs.mkdirSync(dir, { recursive: true });
      for (const name of ["DragentPushService.kt", "DragentPushHandler.kt"]) {
        fs.copyFileSync(path.join(here, "android", name), path.join(dir, name));
      }
      // The agents' icons, the large icon of their notifications.
      const drawables = path.join(root, "app", "src", "main", "res", "drawable-nodpi");
      fs.mkdirSync(drawables, { recursive: true });
      for (const icon of AGENT_ICONS) fs.copyFileSync(path.join(ICON_DIR, icon), path.join(drawables, icon));
      const values = path.join(root, "app", "src", "main", "res", "values");
      fs.mkdirSync(values, { recursive: true });
      fs.writeFileSync(
        path.join(values, "dragent_push_strings.xml"),
        `<?xml version="1.0" encoding="utf-8"?>
<!-- e2ee-nse: the constant push placeholder (docs/design/e2ee.md §8). -->
<resources>
  <string name="dragent_push_placeholder_title" translatable="false">${escapeXml(PLACEHOLDER.title)}</string>
  <string name="dragent_push_placeholder_body">${escapeXml(PLACEHOLDER.body)}</string>
</resources>
`,
      );
      return cfg;
    },
  ]);
}

function withE2eePush(config) {
  config = withExtensionFiles(config);
  config = withExtensionTarget(config);
  config = withAppGroup(config);
  config = withPushService(config);
  config = withFirebaseMessaging(config);
  config = withAndroidFiles(config);
  return config;
}

module.exports = withE2eePush;
module.exports.TARGET = TARGET;
module.exports.PLACEHOLDER = PLACEHOLDER;
module.exports.SHARED_KEYCHAIN_GROUP = SHARED_KEYCHAIN_GROUP;
module.exports.infoPlist = infoPlist;
module.exports.entitlementsPlist = entitlementsPlist;
module.exports.applyPushService = applyPushService;
module.exports.applyFirebaseMessaging = applyFirebaseMessaging;
module.exports.SWIFT_SOURCES = SWIFT_SOURCES;
