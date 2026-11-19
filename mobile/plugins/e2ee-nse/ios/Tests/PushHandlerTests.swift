// §12.8 device cases for the iOS Notification Service Extension (E9a), on a
// simulator: real seals from fixtures/push-fixtures.json (generated with the
// desktop's dragent_e2ee), opened through the real keychain items in the
// shared access group, the real DragentE2EEFFI library and the real
// NotificationService class.
import DragentE2EECore
import Foundation
import UserNotifications
import XCTest

final class PushHandlerTests: XCTestCase {
  static let team = "ABCDE12345"
  let kc = DragentE2EEKeychain(privateGroup: "\(team).dev.dragent.e2ee.testhost", sharedGroup: "\(team).dev.dragent.e2ee")
  var fx: [String: Any] = [:]
  var cases: [String: [String: Any]] = [:]
  var now: UInt64 = 0
  var seenURL: URL!

  override func setUpWithError() throws {
    let url = Bundle.module.url(forResource: "push-fixtures", withExtension: "json", subdirectory: "fixtures")!
    fx = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as! [String: Any]
    cases = fx["cases"] as! [String: [String: Any]]
    now = (fx["now_ms"] as! NSNumber).uint64Value
    for item in DragentE2EEKeychainItem.all where item.shared { try kc.delete(item) }
    try kc.write(.push, hex(fx["push_secret"] as! String))
    var prev = Data()
    var retired = (fx["previous_retired_at_ms"] as! NSNumber).uint64Value.bigEndian
    prev.append(Data(bytes: &retired, count: 8))
    prev.append(hex(fx["previous_push_secret"] as! String))
    try kc.write(.pushPrevious, prev)
    let pins = (fx["pins"] as! [[String: String]]).map { PushPin(desktop: $0["desktop"]!, key: hex($0["key"]!)) }
    try kc.write(.pushPins, try encodePushPins(pins: pins))
    let ctx = try JSONSerialization.data(withJSONObject: ["account": fx["account"] as! String, "phone": fx["phone"] as! String])
    try kc.write(.pushContext, ctx)
    seenURL = FileManager.default.temporaryDirectory.appendingPathComponent("seen-\(UUID().uuidString).json")
  }

  override func tearDownWithError() throws {
    for item in DragentE2EEKeychainItem.all where item.shared { try kc.delete(item) }
    try? FileManager.default.removeItem(at: seenURL)
  }

  func hex(_ s: String) -> Data {
    var out = Data()
    var i = s.startIndex
    while i < s.endIndex {
      let j = s.index(i, offsetBy: 2)
      out.append(UInt8(s[i..<j], radix: 16)!)
      i = j
    }
    return out
  }

  /// The real extension class, opening through the test keychain.
  func service(now t: UInt64? = nil) -> NotificationService {
    let s = NotificationService()
    let kc = self.kc
    s.opener = { sealed, nowMs in try DragentE2EEPushStore.open(sealed: sealed, nowMs: nowMs, keychain: kc) }
    s.seen = DragentSeenIds(url: seenURL)
    s.placeholder = .standard
    let fixed = t ?? now
    s.now = { fixed }
    return s
  }

  /// Run one push through the extension; the server's alert is what APNs
  /// carries (§8 step 3), or `alert` to check it's never shown.
  func deliver(_ blob: String?, via s: NotificationService? = nil, alert: (String, String) = ("Dragent", "An agent needs you")) -> UNNotificationContent {
    let content = UNMutableNotificationContent()
    content.title = alert.0
    content.body = alert.1
    if let blob { content.userInfo = ["aps": ["mutable-content": 1], "e": blob] }
    let request = UNNotificationRequest(identifier: "AbCdEfGhIjKlMnOpQrSt_-", content: content, trigger: nil)
    var out: UNNotificationContent?
    (s ?? service()).didReceive(request) { out = $0 }
    return out!
  }

  func assertPlaceholder(_ c: UNNotificationContent, _ name: String, file: StaticString = #filePath, line: UInt = #line) {
    XCTAssertEqual(c.title, "Dragent", name, file: file, line: line)
    XCTAssertEqual(c.body, "An agent needs you", name, file: file, line: line)
    XCTAssertEqual(c.threadIdentifier, "", name, file: file, line: line)
    XCTAssertNil(c.userInfo["e"], "the sealed blob never reaches the app", file: file, line: line)
    XCTAssertNil(c.userInfo["dragent_desktop"], name, file: file, line: line)
  }

  func testEveryFixtureShowsItsTextOrThePlaceholder() {
    for (name, c) in cases.sorted(by: { $0.key < $1.key }) {
      let out = deliver(c["blob"] as? String)
      switch c["expect"] as! String {
      case "show":
        XCTAssertEqual(out.title, c["title"] as? String, name)
        XCTAssertEqual(out.body, c["body"] as? String, name)
        XCTAssertEqual(out.threadIdentifier, c["thread"] as? String, name)
        XCTAssertEqual(out.userInfo["dragent_desktop"] as? String, c["desktop"] as? String, name)
        XCTAssertNil(out.userInfo["e"], name)
      default:
        // placeholder, and "dismiss" (iOS never gets dismissals: a failure).
        assertPlaceholder(out, name)
      }
    }
  }

  func testValidPushOpensWithTheRealHandler() {
    let c = cases["valid"]!
    let o = DragentPushHandler.handle(
      userInfo: ["e": c["blob"]!], nowMs: now, placeholder: .standard, seen: DragentSeenIds(url: seenURL),
      open: { try DragentE2EEPushStore.open(sealed: $0, nowMs: $1, keychain: self.kc) })
    XCTAssertNil(o.failure)
    XCTAssertEqual(o.notificationId, c["id"] as? String)
    XCTAssertEqual(o.title, "Build finished ✓")
  }

  func testUnpinnedSenderAndMismatchedDesktopIdFail() {
    for name in ["unpinned_sender", "desktop_id_mismatch", "wrong_phone", "tampered", "truncated", "not_base64", "stale"] {
      let o = DragentPushHandler.handle(
        userInfo: ["e": cases[name]!["blob"]!], nowMs: now, placeholder: .standard, seen: DragentSeenIds(url: seenURL),
        open: { try DragentE2EEPushStore.open(sealed: $0, nowMs: $1, keychain: self.kc) })
      XCTAssertNotNil(o.failure, name)
      XCTAssertEqual(o.title, "Dragent", name)
    }
  }

  func testTheAgentsIconComesFromTheTitlesAgentName() {
    XCTAssertEqual(DragentPushHandler.agentIcon(title: "Claude Code · fix-auth"), "claude")
    XCTAssertEqual(DragentPushHandler.agentIcon(title: "Coordination · Auth rewrite"), "dragent")
    XCTAssertNil(DragentPushHandler.agentIcon(title: "Claude Code"), "no workspace part")
    XCTAssertNil(DragentPushHandler.agentIcon(title: "Nightly · build"), "not an agent")
    // Every agent named has its icon in the plugin's icons folder.
    let icons = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
      .deletingLastPathComponent().appendingPathComponent("icons")
    for key in Set(DragentPushHandler.agentIcons.values) {
      XCTAssertTrue(FileManager.default.fileExists(atPath: icons.appendingPathComponent("agent_\(key).png").path), key)
    }
  }

  func testADuplicateIdShowsThePlaceholder() {
    let blob = cases["valid"]!["blob"] as! String
    let s = service()
    XCTAssertEqual(deliver(blob, via: s).title, "Build finished ✓")
    assertPlaceholder(deliver(blob, via: s), "duplicate")
  }

  func testStaleByTheClock() {
    // The valid push read 24 h + 1 s later.
    let blob = cases["valid"]!["blob"] as! String
    assertPlaceholder(deliver(blob, via: service(now: now + 24 * 3_600_000 + 1_000)), "stale")
  }

  func testThePreviousPushKeyWorksForSevenDaysOnly() {
    let blob = cases["previous_push_key"]!["blob"] as! String
    XCTAssertEqual(deliver(blob).title, "Older key")
    // Retired at now - 1 day; 7 days later it's gone (and the push stale anyway
    // by then, so check the key list directly).
    let late = now + 7 * 24 * 3_600_000
    XCTAssertEqual(try DragentE2EEPushStore.pushKeys(kc, nowMs: late).count, 1)
    XCTAssertEqual(try DragentE2EEPushStore.pushKeys(kc, nowMs: now).count, 2)
  }

  func testLockedKeychainBeforeFirstUnlockShowsThePlaceholder() {
    // The simulator can't be "locked before first unlock"; the keychain then
    // answers errSecInteractionNotAllowed, which DragentE2EEKeychain maps to
    // .locked. Inject exactly that error.
    let s = service()
    s.opener = { _, _ in throw DragentE2EEKeychainError.locked }
    assertPlaceholder(deliver(cases["valid"]!["blob"] as? String, via: s), "locked")
  }

  func testNoPushContextOrNoPinsShowsThePlaceholder() throws {
    try kc.delete(.pushContext)
    assertPlaceholder(deliver(cases["valid"]!["blob"] as? String), "no context")
    try setUpWithError()
    // Phone-side unpair removes the desktop's push pin (§12.6).
    try kc.write(.pushPins, try encodePushPins(pins: []))
    assertPlaceholder(deliver(cases["valid"]!["blob"] as? String), "unpaired")
  }

  func testThirtySecondTimeoutShowsThePlaceholderOnce() {
    let s = service()
    let gate = DispatchSemaphore(value: 0)
    s.opener = { _, _ in gate.wait(); throw DragentE2EEKeychainError.locked }
    let content = UNMutableNotificationContent()
    content.title = "server text"
    content.body = "server body"
    content.userInfo = ["e": cases["valid"]!["blob"]!]
    let request = UNNotificationRequest(identifier: "x", content: content, trigger: nil)
    var delivered: [UNNotificationContent] = []
    let lock = NSLock()
    let started = expectation(description: "started")
    DispatchQueue.global().async {
      started.fulfill()
      s.didReceive(request) { c in lock.lock(); delivered.append(c); lock.unlock() }
    }
    wait(for: [started], timeout: 5)
    Thread.sleep(forTimeInterval: 0.1)
    s.serviceExtensionTimeWillExpire()
    gate.signal()
    Thread.sleep(forTimeInterval: 0.2)
    lock.lock()
    defer { lock.unlock() }
    XCTAssertEqual(delivered.count, 1)
    assertPlaceholder(delivered[0], "timeout")
  }

  func testTheServersOwnVisibleTextIsNeverShown() {
    // No blob: a push the server wrote itself.
    assertPlaceholder(deliver(nil, alert: ("Totally Dragent", "Click here")), "no blob")
    // A failing blob with server text.
    assertPlaceholder(deliver(cases["unpinned_sender"]!["blob"] as? String, alert: ("Phish", "Enter your password")), "server text")
  }

  func testPlaceholderComesFromTheBundleOrTheConstants() {
    XCTAssertEqual(DragentPushPlaceholder.fromBundle(Bundle(for: PushHandlerTests.self)), .standard)
    XCTAssertEqual(DragentPushPlaceholder.standard.title, fx["placeholder"].flatMap { ($0 as? [String: String])?["title"] })
  }

  func testSeenListPrunesAfterADay() {
    let seen = DragentSeenIds(url: seenURL)
    XCTAssertTrue(seen.insert(id: "a", issuedAtMs: now, nowMs: now))
    XCTAssertFalse(seen.insert(id: "a", issuedAtMs: now, nowMs: now + 1))
    XCTAssertTrue(seen.insert(id: "a", issuedAtMs: now, nowMs: now + 24 * 3_600_000))
  }

  func testTheAppGroupContainerIsReachable() {
    XCTAssertNotNil(FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: DragentSeenIds.appGroup))
  }

  func testThreadIdentifierIsSha256OfDesktopAndWorkspace() {
    let c = cases["valid"]!
    XCTAssertEqual(
      DragentPushHandler.threadIdentifier(desktop: "0a0b0c0d-0000-4000-8000-0000000000d1", workspace: "0a0b0c0d-0000-4000-8000-0000000000e1"),
      c["thread"] as? String)
  }

  func testPushItemsUseTheSharedGroupAndAfterFirstUnlockClass() throws {
    for item in [DragentE2EEKeychainItem.push, .pushPrevious, .pushPins, .pushContext] {
      let a = try XCTUnwrap(try kc.storedAttributes(item))
      XCTAssertEqual(a.keyClass, .afterFirstUnlockThisDeviceOnly, item.account)
      XCTAssertEqual(a.accessGroup, "\(Self.team).dev.dragent.e2ee", item.account)
      XCTAssertFalse(a.synchronizable)
    }
  }
}
