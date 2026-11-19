// E9a: the push config plugin (plugins/e2ee-nse) and the app config it relies on.
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { join } from "node:path";
import { test } from "node:test";

const require = createRequire(import.meta.url);
const root = join(import.meta.dirname, "..");
const plugin = require("../plugins/e2ee-nse/app.plugin.js") as {
  TARGET: string;
  PLACEHOLDER: { title: string; body: string };
  SHARED_KEYCHAIN_GROUP: string;
  SWIFT_SOURCES: Record<string, string>;
  infoPlist(config: unknown): string;
  entitlementsPlist(): string;
  applyPushService(manifest: Record<string, unknown>): Record<string, any>;
  applyFirebaseMessaging(contents: string): string;
};

test("the extension can reach only the shared keychain group, never the identity's", () => {
  const ent = plugin.entitlementsPlist();
  assert.match(ent, /<string>\$\(AppIdentifierPrefix\)dev\.dragent\.e2ee<\/string>/);
  assert.doesNotMatch(ent, /com\.dragent\.mobile<\/string>\s*<\/array>\s*<key>com\.apple\.security\.application-groups/);
  assert.equal((ent.match(/AppIdentifierPrefix/g) ?? []).length, 1, "exactly one keychain group");
  assert.match(ent, /group\.com\.dragent\.mobile/);
  const info = plugin.infoPlist({ version: "1.2.3", ios: { buildNumber: "45" } });
  assert.match(info, /DragentE2EESharedKeychainGroup<\/key><string>\$\(AppIdentifierPrefix\)dev\.dragent\.e2ee/);
  assert.doesNotMatch(info, /DragentE2EEPrivateKeychainGroup/);
  assert.match(info, /com\.apple\.usernotifications\.service/);
  assert.match(info, /<key>CFBundleShortVersionString<\/key><string>1\.2\.3</);
  assert.match(info, /<key>CFBundleVersion<\/key><string>45</);
});

test("the placeholder is the constant text the gateway sends, and lives in the bundle", () => {
  assert.deepEqual(plugin.PLACEHOLDER, { title: "Dragent", body: "An agent needs you" });
  const info = plugin.infoPlist({});
  assert.match(info, /DragentPushPlaceholderTitle<\/key><string>Dragent</);
  assert.match(info, /DragentPushPlaceholderBody<\/key><string>An agent needs you</);
  const swift = readFileSync(join(root, "plugins/e2ee-nse/ios/DragentPushHandler.swift"), "utf8");
  assert.match(swift, /title: "Dragent", body: "An agent needs you"/);
  const kotlin = readFileSync(join(root, "plugins/e2ee-nse/android/DragentPushService.kt"), "utf8");
  assert.match(kotlin, /"Dragent"/);
  assert.match(kotlin, /"An agent needs you"/);
});

test("every Swift source the extension compiles exists", () => {
  for (const [name, src] of Object.entries(plugin.SWIFT_SOURCES)) assert.ok(existsSync(src), name);
});

test("Android: DragentPushService replaces expo's messaging service, once", () => {
  const manifest = { $: {}, application: [{ $: {}, service: [{ $: { "android:name": "other.Service" } }] }] };
  const once = plugin.applyPushService(manifest);
  const twice = plugin.applyPushService(once);
  const services = twice.application[0].service as { $: Record<string, string>; "intent-filter"?: any[] }[];
  assert.equal(twice.$["xmlns:tools"], "http://schemas.android.com/tools");
  assert.deepEqual(services.map((s) => s.$["android:name"]), [
    "other.Service",
    "expo.modules.notifications.service.ExpoFirebaseMessagingService",
    "dev.dragent.e2ee.push.DragentPushService",
  ]);
  assert.equal(services[1]!.$["tools:node"], "remove");
  assert.equal(services[2]!.$["android:exported"], "false");
  assert.equal(services[2]!["intent-filter"]![0].action[0].$["android:name"], "com.google.firebase.MESSAGING_EVENT");
  const gradle = "android {}\ndependencies {\n    implementation 'x'\n}\n";
  const out = plugin.applyFirebaseMessaging(plugin.applyFirebaseMessaging(gradle));
  assert.equal((out.match(/firebase-messaging/g) ?? []).length, 1);
});

test("app.json registers the plugin and the extension's EAS credentials", () => {
  const app = JSON.parse(readFileSync(join(root, "app.json"), "utf8")).expo;
  assert.ok(app.plugins.includes("./plugins/e2ee-nse/app.plugin.js"));
  assert.ok(app.plugins.indexOf("./plugins/e2ee-nse/app.plugin.js") > app.plugins.findIndex((p: unknown) => Array.isArray(p) && p[0] === "expo-notifications"));
  const ext = app.extra.eas.build.experimental.ios.appExtensions.find((x: { targetName: string }) => x.targetName === plugin.TARGET);
  assert.equal(ext.bundleIdentifier, `${app.ios.bundleIdentifier}.NotificationService`);
  assert.deepEqual(ext.entitlements["keychain-access-groups"], [plugin.SHARED_KEYCHAIN_GROUP]);
  assert.deepEqual(ext.entitlements["com.apple.security.application-groups"], ["group.com.dragent.mobile"]);
});

test("the extension never logs or keeps payload text", () => {
  for (const f of ["ios/NotificationService.swift", "ios/DragentPushHandler.swift", "android/DragentPushService.kt", "android/DragentPushHandler.kt"]) {
    const src = readFileSync(join(root, "plugins/e2ee-nse", f), "utf8");
    assert.doesNotMatch(src, /\b(print|NSLog|os_log|Log\.[dviwe])\(/, f);
  }
});
