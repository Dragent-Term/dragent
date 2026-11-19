# dragent-e2ee: the phone's native crypto module

The phone side of `docs/design/e2ee.md` §3.2 (task E4). It is the Rust crate
`dragent_e2ee` (Noise revision 34 KK / IKpsk1 / X on the §3.2 pins), exposed to
Swift and Kotlin through uniffi, and to JS over JSI through an Expo module.
There is no JavaScript cryptography and no fallback: on the web, or when the
native module is missing, `getE2ee()` throws.

```ts
import { getE2ee, e2eeErrorCode } from "../modules/dragent-e2ee";
const e2ee = getE2ee(); // throws E2EE_UNSUPPORTED_PLATFORM on the web
```

## Layout

| Path | What |
|---|---|
| `rust/` | `dragent_e2ee_ffi`, the uniffi crate. No crypto of its own; wraps the crate. Its own `Cargo.lock`, seeded from `dragent/Cargo.lock`. |
| `ios/Generated/` | uniffi Swift binding + C header/module map (committed; `scripts/build-e2ee-rust.sh check-bindings` fails if stale). |
| `ios/Shared/DragentE2EEKeychain.swift` | Keychain items, classes and access groups, plus `DragentE2EEPushStore.open` for the NSE. No Expo dependency. |
| `ios/DragentE2EEModule.swift` | The Expo module (JSI). |
| `ios/Frameworks/DragentE2EEFFI.xcframework` | Built by `scripts/build-e2ee-rust.sh ios` (gitignored). |
| `android/…/uniffi/` | uniffi Kotlin binding (committed). |
| `android/…/dev/dragent/e2ee/` | Keystore wrapping (`DragentE2EEKeyStore.kt`), push helper (`DragentE2EEPush.kt`), the Expo module. |
| `index.ts`, `src/` | TS API and types; `src/api.ts` has the web refusal and the JS benchmark. |
| `tests/` | Node tests run by `npm test` (config, parity, build-script guards). |
| `ios/Tests/` | Simulator tests (vectors, keychain classes, binding API, benchmarks). |

## Private keys never reach JS

The identity and push key pairs are Rust objects held by the native module,
read from the keychain / Keystore. JS gets public keys and opaque objects
(`KkHandshake`, `PairingHandshake`, `Channel`, `Reassembler`, `PartSession`,
`PinStore`). The pairing secret `P` is passed in once and wiped in Rust;
the caller drops its copy at once (§6.2 step 3).

## Key storage (§6.7)

iOS (service `dev.dragent.e2ee`, generic passwords, never synchronised):

| Item | Class | Access group |
|---|---|---|
| `identity.v1`, `pinstore.key.v1`, `pinstore.gen.v1` | `WhenPasscodeSetThisDeviceOnly` | `<TeamID>.com.dragent.mobile` (app only) |
| `push.v1`, `push.previous.v1`, `pushpins.v1`, `pushcontext.v1` | `AfterFirstUnlockThisDeviceOnly` | `<TeamID>.dev.dragent.e2ee` (shared with the NSE) |

The groups come from the Info.plist keys `DragentE2EEPrivateKeychainGroup` and
`DragentE2EESharedKeychainGroup` (set in `app.json` from
`$(AppIdentifierPrefix)`). A missing or unexpanded value is an error, never the
process's default group.

Android: secrets are AES-256-GCM wrapped by Keystore keys. Identity, pin-store
key and generation use `dragent.e2ee.wrap.id.v1` with
`setUnlockedDeviceRequired(true)` (API 28+; below that, identity creation is
refused with `E2EE_UNSUPPORTED`). Push items use `dragent.e2ee.wrap.push.v1`
with no unlock requirement, so the messaging service can decrypt while locked.
Backup is off (`android.allowBackup: false`).

Pairing is refused without a device passcode (`createIdentityKey` throws
`E2EE_NO_PASSCODE`; `hasDevicePasscode()` lets the UI say so first).

## Fixed ids for the Notification Service Extension (E9a, §8)

- Keychain access group shared with the NSE: `$(AppIdentifierPrefix)dev.dragent.e2ee`.
  The NSE's `keychain-access-groups` entitlement lists only this group, and its
  Info.plist sets `DragentE2EESharedKeychainGroup` to the same value (so the
  identity key is unreachable from the NSE).
- NSE bundle id: `com.dragent.mobile.NotificationService`.
- App Group for the seen-id list: `group.com.dragent.mobile` (already used by the widget).
- In the NSE: compile `ios/Shared/DragentE2EEKeychain.swift` and
  `ios/Generated/dragent_e2ee_ffi.swift`, link `DragentE2EEFFI.xcframework`, and
  call `DragentE2EEPushStore.open(sealed:nowMs:)`. Any throw → the constant
  placeholder from the bundle. The app writes the push context with
  `setPushContext(account, phone)` and the push-pin list with `setPushPin` /
  `removePushPin`.
- Android: `DragentE2EEPush.open(DragentE2EEKeyStore(context), sealed, nowMs)`.

## Building and testing

```sh
cd mobile
npm run e2ee:check        # §3.2 pins + bindings up to date
npm run e2ee:build -- ios # ios/Frameworks/DragentE2EEFFI.xcframework (release, LTO)
npm run e2ee:test:rust    # host tests of the binding
npm run e2ee:test:ios     # simulator: vectors + keychain classes, then benchmarks
npm run typecheck && npm test
```

- `--vectors` builds compile the crate's Noise and codec vector runners in;
  they are debug-only (the crate refuses them with optimisations on) and go to
  `build/vectors/`, never into the app. The build script refuses `--vectors`
  inside an EAS build.
- EAS: `eas-build-pre-install` runs `scripts/build-e2ee-rust.sh eas`, which
  installs Rust (EAS images have none), checks the pins and bindings, and
  builds for `EAS_BUILD_PLATFORM`.
- The simulator tests run in a small host app whose binary carries keychain
  entitlements (an `xctest` agent process has none). They need an iOS simulator
  runtime; `SIMULATOR=<udid>` picks one.

## Benchmarks (§11, §12.12)

`getE2ee().nativeBenchmarks(n)` times on the device: KK handshake, 64-byte
seal/open, 64 KiB TERM open, push open, catalog open. `runBenchmarks(e2ee)`
adds the JSI crossing measured from JS. The simulator bench run asserts the
§11 budget (≤ 0.2 ms per keystroke round trip, ≤ 1 ms per 64 KiB record).

**Provisional results (2026-11-09).** Release library, iOS 26.4 simulator on an
Apple-silicon Mac, 2,000 iterations. The machine was heavily loaded by other
builds (load average 37–143 on 10 cores), and the same code on the Mac host
measured about the same, so these overstate the real cost. Re-run on an idle
machine and a real phone before treating them as the §11 numbers.

| Operation | µs/op | §11 estimate |
|---|---|---|
| seal a 64-byte record | 3.43 | ~1–3 |
| open a 64-byte record | 2.05 | ~1–3 |
| Swift → Rust call, 64-byte buffer | 8.70 | ~5–20 (JSI) |
| open + decode a 64 KiB TERM record | 1,229 | ~60–120 (budget ≤ 1,000) |
| KK handshake, both sides | 1,370 | ~200–500 (phone side) |
| open a push | 350 | ~300 |
| open a 16 KiB catalog | 1,570 | — |

The keystroke path is within budget even under load: a seal, an open and two
calls take about 23 µs against 200 µs. The 64 KiB open missed its 1 ms budget
by 23% under load. It is the one number to confirm when the machine is idle.

## Android: what remains

Written but not built or run here (no Android SDK/NDK on the build machine):
build `jniLibs` with `scripts/build-e2ee-rust.sh android` (NDK + `cargo-ndk`
4.1.2), compile the Kotlin module in an app build, and run the vectors and a
Keystore check (`keyStorageInfo()`) on an emulator (API 28+).
