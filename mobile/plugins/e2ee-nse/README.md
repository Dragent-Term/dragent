# e2ee-nse: end-to-end encrypted push on the phone

The device side of `docs/design/e2ee.md` §8 (task E9a). The desktop seals each
notification with Noise `X` to the phone's push key; the gateway, Apple and
Google see one fixed-size blob and the constant alert "Dragent / An agent
needs you". This plugin adds what opens it on the phone.

| Path | What |
|---|---|
| `app.plugin.js` | Expo config plugin (registered in `app.json`). iOS: adds the `DragentNotificationService` app-extension target. Android: swaps expo-notifications' messaging service for `DragentPushService` and adds `firebase-messaging` to the app module. |
| `ios/NotificationService.swift` | The extension class. Starts from the bundle placeholder, never the server's alert; timeout → placeholder. |
| `ios/DragentPushHandler.swift` | The decision: decode `e`, open (Rust: sender lookup by static key, inner `desktop_id`, 24 h), drop duplicates (App Group seen list), set title, body, `threadIdentifier = hex(SHA-256(desktop ‖ workspace))`. |
| `android/DragentPushService.kt`, `android/DragentPushHandler.kt` | The same for Android: tag = notification id, dismissals cancel by tag, placeholder on failure or after 10 s. Copied into the app at prebuild. |
| `fixtures/push-fixtures.json` | Real seals of every §12.8 case, from `fixtures/gen` (the desktop's `dragent_e2ee`; phones never seal). |

The extension's entitlements list only `$(AppIdentifierPrefix)dev.dragent.e2ee`
(push key, pins, context) and `group.com.dragent.mobile`; the identity key, in
the app's private group, is unreachable from it.

## Tests

```sh
cd mobile
bash plugins/e2ee-nse/ios/Tests/run-simulator-tests.sh   # §12.8 on an iOS simulator
npx tsx --test scripts/e2ee-nse-plugin.test.ts           # plugin output (also in npm test)
```

Regenerate the fixtures (only when the push format changes):

```sh
cargo run --manifest-path plugins/e2ee-nse/fixtures/gen/Cargo.toml --offline > plugins/e2ee-nse/fixtures/push-fixtures.json
```

## Not verified here

- **Android** is written but not built or run: this machine has no Android SDK
  (as for E4's module). Build it in an app build and run the fixtures through
  `DragentPushHandler.handle` on an emulator (API 28+).
- **A real push through the installed app** (`xcrun simctl push` with the
  extension running): this machine's Xcode 26.6 has the iOS 26.5 SDK but only
  the 26.4 simulator runtime, so it offers no simulator destination for a
  full app build. The extension target itself builds and links for the
  simulator from the prebuilt project, and the extension class runs on the
  simulator in the tests above.
- **"Locked before first unlock"** can't be produced on a simulator; the test
  injects the keychain's `.locked` error instead.
