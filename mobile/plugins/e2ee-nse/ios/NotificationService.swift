// The iOS Notification Service Extension (docs/design/e2ee.md §8 step 4).
//
// Every Dragent push arrives with the constant alert "Dragent / An agent
// needs you" and `mutable-content: 1`. This extension replaces it with the
// decrypted title and body, or with the placeholder from its own bundle on
// any failure, including running out of time. The server's alert text is
// never kept: the content always starts from the bundle's placeholder.
//
// No JavaScript runs here. It links the same dragent_e2ee static library
// (DragentE2EEFFI.xcframework) through uniffi and reads the push key, pins
// and context from the keychain group shared with the app.

import Foundation
import UserNotifications

@objc(NotificationService)
public class NotificationService: UNNotificationServiceExtension {
  /// Replaced by tests; the shared-keychain open otherwise.
  public var opener: (Data, UInt64) throws -> PushNotification = { sealed, nowMs in
    try DragentE2EEPushStore.open(sealed: sealed, nowMs: nowMs)
  }
  public var seen: DragentSeenIds = .shared()
  public var placeholder: DragentPushPlaceholder = .fromBundle(Bundle(for: NotificationService.self))
  public var now: () -> UInt64 = { UInt64(Date().timeIntervalSince1970 * 1000) }

  private let lock = NSLock()
  private var handler: ((UNNotificationContent) -> Void)?
  private var fallback: UNNotificationContent?

  private func deliver(_ content: UNNotificationContent) {
    lock.lock()
    let h = handler
    handler = nil
    lock.unlock()
    h?(content)
  }

  func content(from outcome: DragentPushOutcome, base: UNNotificationContent) -> UNMutableNotificationContent {
    let c = (base.mutableCopy() as? UNMutableNotificationContent) ?? UNMutableNotificationContent()
    c.title = outcome.title
    c.subtitle = ""
    c.body = outcome.body
    c.threadIdentifier = outcome.threadIdentifier ?? ""
    // Drop the sealed blob and anything else the server sent; keep only
    // what the app needs to open the right desktop on tap.
    var info: [AnyHashable: Any] = [:]
    if let desktop = outcome.desktopId { info["dragent_desktop"] = desktop }
    // Allow / Deny on the notification (the app registers the category).
    if let pane = outcome.answerPane, outcome.desktopId != nil {
      info["dragent_pane"] = pane
      c.categoryIdentifier = "dragent-answer"
    }
    c.userInfo = info
    // The agent's icon as the notification's picture.
    if let key = DragentPushHandler.agentIcon(title: outcome.title), let attachment = agentAttachment(key) {
      c.attachments = [attachment]
    }
    return c
  }

  /// A copy of the bundled `agent_<key>.png`: iOS moves an attachment's
  /// file into its own store, so the bundle's original is never handed over.
  func agentAttachment(_ key: String) -> UNNotificationAttachment? {
    guard let source = Bundle(for: NotificationService.self).url(forResource: "agent_\(key)", withExtension: "png") else {
      return nil
    }
    let copy = FileManager.default.temporaryDirectory.appendingPathComponent("\(UUID().uuidString)-agent_\(key).png")
    do {
      try FileManager.default.copyItem(at: source, to: copy)
      return try UNNotificationAttachment(identifier: "agent", url: copy, options: nil)
    } catch {
      return nil
    }
  }

  public override func didReceive(
    _ request: UNNotificationRequest,
    withContentHandler contentHandler: @escaping (UNNotificationContent) -> Void
  ) {
    let failed = DragentPushOutcome.placeholder(placeholder, .timeout)
    lock.lock()
    handler = contentHandler
    fallback = content(from: failed, base: request.content)
    lock.unlock()
    let outcome = DragentPushHandler.handle(
      userInfo: request.content.userInfo,
      nowMs: now(),
      placeholder: placeholder,
      seen: seen,
      open: opener
    )
    deliver(content(from: outcome, base: request.content))
  }

  /// §8 step 4.5: out of time → the placeholder, never the payload.
  public override func serviceExtensionTimeWillExpire() {
    lock.lock()
    let f = fallback
    lock.unlock()
    if let f { deliver(f) }
  }
}
