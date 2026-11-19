// Phone key storage for end-to-end encryption (docs/design/e2ee.md §6.1, §6.7, §8).
//
// Shared by the app's Expo module and the Notification Service Extension
// (E9a): no Expo or React Native dependency. Compile this file, the generated
// `dragent_e2ee_ffi.swift` and link `DragentE2EEFFI.xcframework`.
//
// Items (service "dev.dragent.e2ee", generic passwords, never synchronised):
//
// | account            | contents                         | class (§6.7)                         | group   |
// |--------------------|----------------------------------|--------------------------------------|---------|
// | identity.v1        | X25519 identity secret (32)      | WhenPasscodeSetThisDeviceOnly        | private |
// | pinstore.key.v1    | pin-store state key (32)         | WhenPasscodeSetThisDeviceOnly        | private |
// | pinstore.gen.v1    | pin-store generation (u64 BE)    | WhenPasscodeSetThisDeviceOnly        | private |
// | push.v1            | X25519 push secret (32)          | AfterFirstUnlockThisDeviceOnly       | shared  |
// | push.previous.v1   | retired_at_ms (u64 BE) + secret  | AfterFirstUnlockThisDeviceOnly       | shared  |
// | pushpins.v1        | push-pin list (encodePushPins)   | AfterFirstUnlockThisDeviceOnly       | shared  |
// | pushcontext.v1     | {"account","phone"} JSON         | AfterFirstUnlockThisDeviceOnly       | shared  |
//
// The private group is the app's own (`<TeamID>.com.dragent.mobile`); the
// shared group `<TeamID>.dev.dragent.e2ee` is also in the NSE's
// keychain-access-groups. Both come from Info.plist keys that app.json fills
// with `$(AppIdentifierPrefix)…`, so the Team ID is never hard-coded.

import Foundation
import Security

public enum DragentE2EEKeychainClass: String {
  /// §6.7 identity class: readable only while unlocked, only with a passcode, never backed up.
  case whenPasscodeSetThisDeviceOnly
  /// §6.7 push class: readable after the first unlock, so the NSE works while locked.
  case afterFirstUnlockThisDeviceOnly
  case other

  var secValue: CFString {
    switch self {
    case .whenPasscodeSetThisDeviceOnly: return kSecAttrAccessibleWhenPasscodeSetThisDeviceOnly
    case .afterFirstUnlockThisDeviceOnly: return kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
    case .other: return kSecAttrAccessibleWhenUnlockedThisDeviceOnly
    }
  }

  init(secValue: String) {
    if secValue == kSecAttrAccessibleWhenPasscodeSetThisDeviceOnly as String {
      self = .whenPasscodeSetThisDeviceOnly
    } else if secValue == kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly as String {
      self = .afterFirstUnlockThisDeviceOnly
    } else {
      self = .other
    }
  }
}

public enum DragentE2EEKeychainError: Error, Equatable {
  /// The item's class needs a passcode and the device has none (§6.2 step 4: refuse pairing).
  case noPasscode
  /// The device is locked (or not yet unlocked since boot) for this item's class.
  case locked
  /// The access group isn't configured (missing Info.plist key / entitlement).
  case notConfigured(String)
  case status(OSStatus)
}

public struct DragentE2EEKeychainItem: Hashable {
  public let account: String
  public let keyClass: DragentE2EEKeychainClass
  public let shared: Bool

  public static let identity = Self(account: "identity.v1", keyClass: .whenPasscodeSetThisDeviceOnly, shared: false)
  public static let pinStoreKey = Self(account: "pinstore.key.v1", keyClass: .whenPasscodeSetThisDeviceOnly, shared: false)
  public static let pinStoreGeneration = Self(account: "pinstore.gen.v1", keyClass: .whenPasscodeSetThisDeviceOnly, shared: false)
  public static let push = Self(account: "push.v1", keyClass: .afterFirstUnlockThisDeviceOnly, shared: true)
  public static let pushPrevious = Self(account: "push.previous.v1", keyClass: .afterFirstUnlockThisDeviceOnly, shared: true)
  public static let pushPins = Self(account: "pushpins.v1", keyClass: .afterFirstUnlockThisDeviceOnly, shared: true)
  public static let pushContext = Self(account: "pushcontext.v1", keyClass: .afterFirstUnlockThisDeviceOnly, shared: true)

  public static let all: [Self] = [.identity, .pinStoreKey, .pinStoreGeneration, .push, .pushPrevious, .pushPins, .pushContext]
}

public struct DragentE2EEKeychain {
  public static let service = "dev.dragent.e2ee"
  /// Info.plist key holding `$(AppIdentifierPrefix)dev.dragent.e2ee`.
  public static let sharedGroupInfoKey = "DragentE2EESharedKeychainGroup"
  /// Info.plist key holding `$(AppIdentifierPrefix)com.dragent.mobile` (app only).
  public static let privateGroupInfoKey = "DragentE2EEPrivateKeychainGroup"

  public let privateGroup: String?
  public let sharedGroup: String?
  public let service: String
  /// Strict (the app and the NSE): a missing group is an error, never the
  /// process's default group, which could put the identity key in a group the
  /// NSE shares. Only tests construct a non-strict keychain.
  public let strict: Bool

  public init(privateGroup: String?, sharedGroup: String?, service: String = DragentE2EEKeychain.service, strict: Bool = false) {
    self.privateGroup = privateGroup
    self.sharedGroup = sharedGroup
    self.service = service
    self.strict = strict
  }

  /// The configuration from the bundle's Info.plist. Unexpanded build
  /// variables count as missing (fail closed rather than write to a wrong group).
  public static func fromBundle(_ bundle: Bundle = .main) -> DragentE2EEKeychain {
    func group(_ key: String) -> String? {
      guard let v = bundle.object(forInfoDictionaryKey: key) as? String, !v.isEmpty, !v.contains("$(") else {
        return nil
      }
      return v
    }
    return DragentE2EEKeychain(privateGroup: group(privateGroupInfoKey), sharedGroup: group(sharedGroupInfoKey), strict: true)
  }

  private func baseQuery(_ item: DragentE2EEKeychainItem) throws -> [CFString: Any] {
    var q: [CFString: Any] = [
      kSecClass: kSecClassGenericPassword,
      kSecAttrService: service,
      kSecAttrAccount: item.account,
      kSecAttrSynchronizable: kCFBooleanFalse as Any,
    ]
    let group = item.shared ? sharedGroup : privateGroup
    if let group {
      q[kSecAttrAccessGroup] = group
    } else if strict {
      throw DragentE2EEKeychainError.notConfigured(item.shared ? Self.sharedGroupInfoKey : Self.privateGroupInfoKey)
    }
    return q
  }

  private static func map(_ status: OSStatus) -> DragentE2EEKeychainError {
    switch status {
    case errSecInteractionNotAllowed: return .locked
    case errSecNotAvailable, errSecAuthFailed: return .noPasscode
    default: return .status(status)
    }
  }

  public func read(_ item: DragentE2EEKeychainItem) throws -> Data? {
    var q = try baseQuery(item)
    q[kSecReturnData] = true
    q[kSecMatchLimit] = kSecMatchLimitOne
    var out: CFTypeRef?
    let status = SecItemCopyMatching(q as CFDictionary, &out)
    switch status {
    case errSecSuccess: return out as? Data
    case errSecItemNotFound: return nil
    default: throw Self.map(status)
    }
  }

  /// Writes with the item's §6.7 class. Updates in place when present, so a
  /// crash can never leave the item deleted but not re-added.
  public func write(_ item: DragentE2EEKeychainItem, _ data: Data) throws {
    let q = try baseQuery(item)
    let attrs: [CFString: Any] = [
      kSecValueData: data,
      kSecAttrAccessible: item.keyClass.secValue,
    ]
    var status = SecItemUpdate(q as CFDictionary, attrs as CFDictionary)
    if status == errSecItemNotFound {
      var add = q
      for (k, v) in attrs { add[k] = v }
      status = SecItemAdd(add as CFDictionary, nil)
    }
    guard status == errSecSuccess else { throw Self.map(status) }
  }

  public func delete(_ item: DragentE2EEKeychainItem) throws {
    let status = SecItemDelete(try baseQuery(item) as CFDictionary)
    guard status == errSecSuccess || status == errSecItemNotFound else { throw Self.map(status) }
  }

  /// The stored item's protection class and access group, read back from the
  /// keychain (asserted by the simulator test and shown in diagnostics).
  public func storedAttributes(_ item: DragentE2EEKeychainItem) throws -> (keyClass: DragentE2EEKeychainClass, accessGroup: String?, synchronizable: Bool)? {
    var q = try baseQuery(item)
    q[kSecReturnAttributes] = true
    q[kSecMatchLimit] = kSecMatchLimitOne
    var out: CFTypeRef?
    let status = SecItemCopyMatching(q as CFDictionary, &out)
    if status == errSecItemNotFound { return nil }
    guard status == errSecSuccess, let attrs = out as? [String: Any] else { throw Self.map(status) }
    let cls = DragentE2EEKeychainClass(secValue: attrs[kSecAttrAccessible as String] as? String ?? "")
    let sync = (attrs[kSecAttrSynchronizable as String] as? NSNumber)?.boolValue ?? false
    return (cls, attrs[kSecAttrAccessGroup as String] as? String, sync)
  }
}

// MARK: - Push (app and NSE)

public enum DragentE2EEPushStore {
  /// §6.6: a retired push key opens notifications for 7 more days.
  public static let previousKeyGraceMs: UInt64 = 7 * 24 * 60 * 60 * 1000

  static func u64(_ v: UInt64) -> Data {
    var be = v.bigEndian
    return Data(bytes: &be, count: 8)
  }

  static func readU64(_ d: Data) -> UInt64? {
    guard d.count >= 8 else { return nil }
    return d.prefix(8).reduce(0) { ($0 << 8) | UInt64($1) }
  }

  /// The push keys to try, current first, then a previous one still in its grace window.
  public static func pushKeys(_ kc: DragentE2EEKeychain, nowMs: UInt64) throws -> [Keypair] {
    var keys: [Keypair] = []
    if var cur = try kc.read(.push) {
      defer { cur.resetBytes(in: 0..<cur.count) }
      keys.append(try Keypair.fromSecret(secret: cur))
    }
    if var prev = try kc.read(.pushPrevious), prev.count == 40, let retired = readU64(prev) {
      defer { prev.resetBytes(in: 0..<prev.count) }
      if nowMs >= retired && nowMs - retired < previousKeyGraceMs {
        keys.append(try Keypair.fromSecret(secret: prev.subdata(in: 8..<40)))
      }
    }
    return keys
  }

  public static func pins(_ kc: DragentE2EEKeychain) throws -> [PushPin] {
    guard let d = try kc.read(.pushPins) else { return [] }
    return try decodePushPins(bytes: d)
  }

  public static func context(_ kc: DragentE2EEKeychain) throws -> (account: String, phone: String)? {
    guard let d = try kc.read(.pushContext),
          let o = try JSONSerialization.jsonObject(with: d) as? [String: String],
          let a = o["account"], let p = o["phone"] else { return nil }
    return (a, p)
  }

  /// §8 step 4: open a sealed push with the stored push keys, pins and context.
  /// Throws on any failure: the caller shows the constant placeholder from the
  /// app bundle. Duplicate ids are the caller's check.
  public static func open(sealed: Data, nowMs: UInt64, keychain kc: DragentE2EEKeychain = .fromBundle()) throws -> PushNotification {
    guard let ctx = try context(kc) else { throw DragentE2EEKeychainError.notConfigured("push context") }
    return try openPush(
      pushKeys: try pushKeys(kc, nowMs: nowMs),
      account: ctx.account,
      phone: ctx.phone,
      sealed: sealed,
      pins: try pins(kc),
      nowMs: nowMs
    )
  }
}
