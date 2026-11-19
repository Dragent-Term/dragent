// §6.7 key classes, asserted by reading the stored items back from the keychain.
import XCTest
@testable import DragentE2EECore

final class KeychainTests: XCTestCase {
  // The host app is signed with these two groups (run-simulator-tests.sh), the
  // same shape as the app: its own group plus the group shared with the NSE.
  // A per-run service keeps runs apart.
  static let teamPrefix = "ABCDE12345."
  let kc = DragentE2EEKeychain(
    privateGroup: teamPrefix + "dev.dragent.e2ee.testhost",
    sharedGroup: teamPrefix + "dev.dragent.e2ee",
    service: "dev.dragent.e2ee.test.\(UUID().uuidString)"
  )

  override func tearDown() {
    for item in DragentE2EEKeychainItem.all { try? kc.delete(item) }
  }

  func testItemTableMatchesTheDesign() {
    XCTAssertEqual(DragentE2EEKeychainItem.identity.keyClass, .whenPasscodeSetThisDeviceOnly)
    XCTAssertEqual(DragentE2EEKeychainItem.pinStoreKey.keyClass, .whenPasscodeSetThisDeviceOnly)
    XCTAssertEqual(DragentE2EEKeychainItem.pinStoreGeneration.keyClass, .whenPasscodeSetThisDeviceOnly)
    XCTAssertFalse(DragentE2EEKeychainItem.identity.shared)
    for item in [DragentE2EEKeychainItem.push, .pushPrevious, .pushPins, .pushContext] {
      XCTAssertEqual(item.keyClass, .afterFirstUnlockThisDeviceOnly, item.account)
      XCTAssertTrue(item.shared, item.account)
    }
  }

  /// The push items are stored AfterFirstUnlockThisDeviceOnly and never synchronised.
  func testPushKeyIsStoredWithTheAfterFirstUnlockClass() throws {
    let k = try Keypair.generate()
    try kc.write(.push, k.secretBytes())
    let a = try XCTUnwrap(try kc.storedAttributes(.push))
    XCTAssertEqual(a.keyClass, .afterFirstUnlockThisDeviceOnly)
    XCTAssertFalse(a.synchronizable)
    XCTAssertEqual(a.accessGroup, Self.teamPrefix + "dev.dragent.e2ee", "push items live in the group shared with the NSE")
    print("[e2ee-test] push key stored with class \(a.keyClass.rawValue) in \(a.accessGroup ?? "-")")
    XCTAssertEqual(try kc.read(.push), k.secretBytes())
    // Updating in place keeps the class.
    try kc.write(.push, try Keypair.generate().secretBytes())
    XCTAssertEqual(try kc.storedAttributes(.push)?.keyClass, .afterFirstUnlockThisDeviceOnly)
  }

  /// The identity key is stored WhenPasscodeSetThisDeviceOnly. A device with
  /// no passcode refuses the item outright (§6.2 step 4: no pairing without a
  /// passcode); the simulator reports which case applies.
  func testIdentityKeyIsStoredWithThePasscodeClassOrRefused() throws {
    let k = try Keypair.generate()
    do {
      try kc.write(.identity, k.secretBytes())
      let a = try XCTUnwrap(try kc.storedAttributes(.identity))
      XCTAssertEqual(a.keyClass, .whenPasscodeSetThisDeviceOnly)
      XCTAssertFalse(a.synchronizable)
      XCTAssertEqual(a.accessGroup, Self.teamPrefix + "dev.dragent.e2ee.testhost", "the identity key is never in the shared group")
      print("[e2ee-test] identity stored with class \(a.keyClass.rawValue) in \(a.accessGroup ?? "-")")
    } catch DragentE2EEKeychainError.noPasscode {
      print("[e2ee-test] identity refused: no passcode on this device (expected without one)")
      XCTAssertNil(try kc.read(.identity))
    }
  }

  func testDeleteAndMissingItems() throws {
    XCTAssertNil(try kc.read(.pushPins))
    try kc.write(.pushPins, try encodePushPins(pins: []))
    XCTAssertNotNil(try kc.read(.pushPins))
    try kc.delete(.pushPins)
    try kc.delete(.pushPins)
    XCTAssertNil(try kc.read(.pushPins))
  }

  func testUnexpandedInfoPlistGroupsCountAsMissing() {
    // The host's Info.plist has the app's unexpanded `$(AppIdentifierPrefix)…`
    // values: they must count as missing (fail closed), never be used verbatim.
    let kc = DragentE2EEKeychain.fromBundle(.main)
    XCTAssertNil(kc.sharedGroup)
    XCTAssertNil(kc.privateGroup)
    XCTAssertTrue(kc.strict)
    XCTAssertThrowsError(try kc.write(.pushPins, Data())) { e in
      XCTAssertEqual(e as? DragentE2EEKeychainError, .notConfigured(DragentE2EEKeychain.sharedGroupInfoKey))
    }
    XCTAssertThrowsError(try kc.read(.identity)) { e in
      XCTAssertEqual(e as? DragentE2EEKeychainError, .notConfigured(DragentE2EEKeychain.privateGroupInfoKey))
    }
  }

  /// The identity key can't be read through the shared group the NSE uses.
  func testIdentityIsNotVisibleInTheSharedGroup() throws {
    try kc.write(.pushPins, try encodePushPins(pins: []))
    let sharedOnly = DragentE2EEKeychain(privateGroup: kc.sharedGroup, sharedGroup: kc.sharedGroup, service: kc.service, strict: true)
    do {
      try kc.write(.identity, try Keypair.generate().secretBytes())
      XCTAssertNil(try sharedOnly.read(.identity))
      XCTAssertNotNil(try kc.read(.identity))
    } catch DragentE2EEKeychainError.noPasscode {
      XCTAssertNil(try sharedOnly.read(.identity))
    }
  }

  /// §6.6: the previous push key opens notifications for 7 days, then not.
  func testPreviousPushKeyGraceWindow() throws {
    let cur = try Keypair.generate()
    let prev = try Keypair.generate()
    try kc.write(.push, cur.secretBytes())
    var blob = DragentE2EEPushStore.u64(1_000)
    blob.append(prev.secretBytes())
    try kc.write(.pushPrevious, blob)
    let within = try DragentE2EEPushStore.pushKeys(kc, nowMs: 1_000 + DragentE2EEPushStore.previousKeyGraceMs - 1)
    XCTAssertEqual(within.map { $0.publicKey() }, [cur.publicKey(), prev.publicKey()])
    let after = try DragentE2EEPushStore.pushKeys(kc, nowMs: 1_000 + DragentE2EEPushStore.previousKeyGraceMs)
    XCTAssertEqual(after.map { $0.publicKey() }, [cur.publicKey()])
  }

  /// No context or pins: every push fails, so the NSE shows the placeholder.
  func testPushOpenFailsClosedWithoutPins() throws {
    try kc.write(.push, try Keypair.generate().secretBytes())
    XCTAssertThrowsError(try DragentE2EEPushStore.open(sealed: Data(count: 1120), nowMs: 0, keychain: kc))
    try kc.write(.pushContext, Data(#"{"account":"11111111-2222-4333-8444-555555555555","phone":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"}"#.utf8))
    XCTAssertThrowsError(try DragentE2EEPushStore.open(sealed: Data(count: 1120), nowMs: 0, keychain: kc)) { e in
      XCTAssertNotNil(e as? E2eeError)
    }
  }
}
