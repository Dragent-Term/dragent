// Release configuration checks for the E2EE phone module (docs/design/e2ee.md
// §1.3, §6.7, §10). Run by `npm test`, so CI fails if any of these regress.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";

const mobile = join(import.meta.dirname, "..", "..", "..");
const read = (p: string) => readFileSync(join(mobile, p), "utf8");
const pkg = JSON.parse(read("package.json")) as {
  scripts: Record<string, string>;
  dependencies: Record<string, string>;
  devDependencies?: Record<string, string>;
};
const app = (JSON.parse(read("app.json")) as { expo: Record<string, any> }).expo;

test("expo-updates is not installed: no over-the-air JS updates (§1.3)", () => {
  const deps = { ...pkg.dependencies, ...pkg.devDependencies };
  assert.equal(deps["expo-updates"], undefined, "package.json must not depend on expo-updates");
  const lock = read("pnpm-lock.yaml");
  // Exact package name: `expo-updates-interface` (a type-only dependency of
  // expo-dev-client) is not the updater.
  assert.doesNotMatch(lock, /^\s*'?expo-updates'?:/m, "pnpm-lock.yaml lists expo-updates as a dependency");
  assert.doesNotMatch(lock, /^\s*'?expo-updates@/m, "pnpm-lock.yaml resolves an expo-updates package");
  assert.equal(app.updates?.url, undefined, "app.json must not configure an update URL");
  assert.notEqual(app.updates?.enabled, true, "app.json must not enable updates");
});

test("the web platform is gone (§1.3, §10)", () => {
  assert.deepEqual(app.platforms, ["ios", "android"]);
  assert.equal(app.web, undefined, "app.json has a web section");
  assert.equal(pkg.scripts.web, undefined, "package.json has a web script");
  assert.equal(pkg.dependencies["react-native-web"], undefined, "react-native-web is a direct dependency");
  assert.equal(app.extra?.router?.asyncRoutes?.web, undefined, "router async routes still target web");
});

test("iOS keychain groups: the app's own plus the group shared with the NSE (§6.7, §8)", () => {
  const groups = app.ios.entitlements["keychain-access-groups"] as string[];
  assert.deepEqual(groups, ["$(AppIdentifierPrefix)com.dragent.mobile", "$(AppIdentifierPrefix)dev.dragent.e2ee"]);
  // The module reads the groups from these Info.plist keys; they must be the
  // same groups, filled by Xcode from the Team ID (never hard-coded).
  assert.equal(app.ios.infoPlist.DragentE2EEPrivateKeychainGroup, groups[0]);
  assert.equal(app.ios.infoPlist.DragentE2EESharedKeychainGroup, groups[1]);
  const swift = read("modules/dragent-e2ee/ios/Shared/DragentE2EEKeychain.swift");
  assert.match(swift, /sharedGroupInfoKey = "DragentE2EESharedKeychainGroup"/);
  assert.match(swift, /privateGroupInfoKey = "DragentE2EEPrivateKeychainGroup"/);
});

test("§6.7 classes in the native key stores", () => {
  const swift = read("modules/dragent-e2ee/ios/Shared/DragentE2EEKeychain.swift");
  for (const item of ["identity", "pinStoreKey", "pinStoreGeneration"]) {
    assert.match(swift, new RegExp(`static let ${item} = Self\\(account: "[^"]+", keyClass: \\.whenPasscodeSetThisDeviceOnly, shared: false\\)`), item);
  }
  for (const item of ["push", "pushPrevious", "pushPins", "pushContext"]) {
    assert.match(swift, new RegExp(`static let ${item} = Self\\(account: "[^"]+", keyClass: \\.afterFirstUnlockThisDeviceOnly, shared: true\\)`), item);
  }
  assert.match(swift, /kSecAttrSynchronizable: kCFBooleanFalse/);
  const kt = read("modules/dragent-e2ee/android/src/main/java/dev/dragent/e2ee/DragentE2EEKeyStore.kt");
  assert.match(kt, /IDENTITY\("dragent\.e2ee\.wrap\.id\.v1", true\)/);
  assert.match(kt, /PUSH\("dragent\.e2ee\.wrap\.push\.v1", false\)/);
  assert.match(kt, /setUnlockedDeviceRequired\(true\)/);
  for (const item of ["IDENTITY", "PIN_STORE_KEY", "PIN_STORE_GENERATION"]) {
    assert.match(kt, new RegExp(`${item}\\("[^"]+", WrapClass\\.IDENTITY\\)`), item);
  }
  for (const item of ["PUSH", "PUSH_PREVIOUS", "PUSH_PINS", "PUSH_CONTEXT"]) {
    assert.match(kt, new RegExp(`${item}\\("[^"]+", WrapClass\\.PUSH\\)`), item);
  }
  assert.equal(app.android.allowBackup, false, "Android backup must be off (keys and pin store)");
});

test("the native library is built from source on EAS, never with the vector runners", () => {
  assert.equal(pkg.scripts["eas-build-pre-install"], "bash scripts/build-e2ee-rust.sh eas");
  const sh = read("scripts/build-e2ee-rust.sh");
  assert.match(sh, /--vectors is for simulator tests only, never an EAS build/);
  const selftest = read("modules/dragent-e2ee/rust/src/selftest.rs");
  assert.match(selftest, /#\[cfg\(not\(debug_assertions\)\)\]\ncompile_error!/);
});
