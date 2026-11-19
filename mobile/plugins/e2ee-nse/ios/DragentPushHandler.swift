// What the Notification Service Extension shows for one push
// (docs/design/e2ee.md §8 step 4). No UI and no UserNotifications here, so
// the simulator tests run it directly.
//
// - The sealed blob is the payload's "e" string (1,494 base64url chars).
// - It is opened with the push key(s), pins and context from the shared
//   keychain group (`DragentE2EEPushStore.open`): decryption, sender lookup
//   by static key, the inner desktop_id check and the 24-hour staleness
//   check all happen in Rust.
// - Duplicate ids are dropped with a seen-id list in the App Group container
//   (not secret: tampering only causes duplicates).
// - Title, body and threadIdentifier = hex(SHA-256(desktop || workspace))
//   are set locally.
// - ANY failure shows the constant placeholder from this bundle's
//   Info.plist, never text from the payload or the server's alert.
// - iOS never receives dismissals (§8 step 6); one arriving is a failure.

import CryptoKit
import Foundation

/// The constant text shown when a push can't be opened.
public struct DragentPushPlaceholder: Equatable {
  public static let titleInfoKey = "DragentPushPlaceholderTitle"
  public static let bodyInfoKey = "DragentPushPlaceholderBody"
  public static let standard = DragentPushPlaceholder(title: "Dragent", body: "An agent needs you")

  public let title: String
  public let body: String

  public init(title: String, body: String) {
    self.title = title
    self.body = body
  }

  /// From the extension's own Info.plist; the built-in constants otherwise.
  public static func fromBundle(_ bundle: Bundle = .main) -> DragentPushPlaceholder {
    let title = bundle.object(forInfoDictionaryKey: titleInfoKey) as? String
    let body = bundle.object(forInfoDictionaryKey: bodyInfoKey) as? String
    guard let title, !title.isEmpty, let body, !body.isEmpty else { return .standard }
    return DragentPushPlaceholder(title: title, body: body)
  }
}

/// Why a push showed the placeholder (for tests; never shown or logged with content).
public enum DragentPushFailure: String, Equatable {
  case missingBlob
  case badEncoding
  case openFailed
  case dismissal
  case duplicate
  case timeout
}

public struct DragentPushOutcome: Equatable {
  public let title: String
  public let body: String
  public let threadIdentifier: String?
  /// The opened notification's id (hex), for tests.
  public let notificationId: String?
  public let desktopId: String?
  public let failure: DragentPushFailure?
  /// An agent's pane waiting on a permission prompt: the notification
  /// offers Allow and Deny, answered through the app (`agent.answer`).
  public var answerPane: String? = nil

  public static func placeholder(_ p: DragentPushPlaceholder, _ why: DragentPushFailure) -> DragentPushOutcome {
    DragentPushOutcome(title: p.title, body: p.body, threadIdentifier: nil, notificationId: nil, desktopId: nil, failure: why)
  }
}

/// The seen-id list (§8 step 4.3): hex id → issued_at_ms, kept for 24 hours.
public final class DragentSeenIds {
  public static let appGroup = "group.com.dragent.mobile"
  static let fileName = "dragent-push-seen.v1.json"
  static let maxEntries = 1024
  static let keepMs: UInt64 = 24 * 60 * 60 * 1000

  private let url: URL?

  public init(url: URL?) {
    self.url = url
  }

  /// The list in the App Group container shared with the app.
  public static func shared(group: String = appGroup) -> DragentSeenIds {
    let dir = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: group)
    return DragentSeenIds(url: dir?.appendingPathComponent(fileName))
  }

  private func load() -> [String: UInt64] {
    guard let url, let data = try? Data(contentsOf: url),
          let raw = try? JSONSerialization.jsonObject(with: data) as? [String: NSNumber] else { return [:] }
    return raw.mapValues { $0.uint64Value }
  }

  /// Records `id`; false when it was already seen in the last 24 hours. An
  /// unavailable container dedupes nothing (duplicates are the only cost).
  public func insert(id: String, issuedAtMs: UInt64, nowMs: UInt64) -> Bool {
    var seen = load().filter { nowMs < $0.value || nowMs - $0.value < Self.keepMs }
    if seen[id] != nil { return false }
    seen[id] = issuedAtMs
    if seen.count > Self.maxEntries {
      for (key, _) in seen.sorted(by: { $0.value < $1.value }).prefix(seen.count - Self.maxEntries) {
        seen.removeValue(forKey: key)
      }
    }
    if let url, let data = try? JSONSerialization.data(withJSONObject: seen.mapValues { NSNumber(value: $0) }) {
      try? data.write(to: url, options: .atomic)
    }
    return true
  }
}

public enum DragentPushHandler {
  /// The payload key carrying the sealed blob (§8 step 3).
  public static let blobKey = "e"

  /// base64url without padding, strictly (anything else is nil).
  public static func decodeBase64Url(_ text: String) -> Data? {
    guard !text.isEmpty, text.utf8.allSatisfy({ c in
      (c >= 0x41 && c <= 0x5A) || (c >= 0x61 && c <= 0x7A) || (c >= 0x30 && c <= 0x39) || c == 0x2D || c == 0x5F
    }), text.count % 4 != 1 else { return nil }
    var s = text.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
    while s.count % 4 != 0 { s.append("=") }
    return Data(base64Encoded: s)
  }

  static func uuidBytes(_ s: String) -> Data? {
    let hex = s.replacingOccurrences(of: "-", with: "")
    guard hex.count == 32 else { return nil }
    var out = Data(capacity: 16)
    var i = hex.startIndex
    while i < hex.endIndex {
      let j = hex.index(i, offsetBy: 2)
      guard let b = UInt8(hex[i..<j], radix: 16) else { return nil }
      out.append(b)
      i = j
    }
    return out
  }

  static func hex(_ d: Data) -> String { d.map { String(format: "%02x", $0) }.joined() }

  /// The desktop titles an agent's notification "<agent> · <workspace>"
  /// (agent names as the desktop shows them). The agent's icon, by the name
  /// before the " · ", is bundled as `agent_<key>.png`; nil when the title
  /// names no agent the app has an icon for.
  public static let agentIcons: [String: String] = [
    "Dragent": "dragent", "Coordination": "dragent", "Claude Code": "claude", "Codex": "codex", "Gemini CLI": "gemini",
    "Gemini": "gemini", "Copilot CLI": "copilot", "OpenCode": "opencode", "Amp": "amp",
    "Droid": "droid", "Grok CLI": "grok", "Devin": "devin", "Kimi": "kimi", "Kiro (AWS)": "kiro",
    "Pi": "pi", "Oh My Pi": "pi", "Mistral Vibe": "vibe", "Hermes Agent": "hermes", "Muse Code": "muse",
    "Goose": "goose", "Cline": "cline", "Codebuff": "codebuff", "Continue": "continue",
    "Kilocode": "kilocode", "Crush": "crush", "Rovo Dev": "rovo", "Junie": "junie", "Antigravity": "antigravity",
  ]

  public static func agentIcon(title: String) -> String? {
    guard let range = title.range(of: " · ") else { return nil }
    return agentIcons[String(title[..<range.lowerBound])]
  }

  /// §8 step 4.4: threadIdentifier = SHA-256(desktop || workspace), hex.
  /// "Needs you" (kind 2) about a real pane: answerable.
  public static func answerPane(kind: UInt8, tab: String) -> String? {
    guard kind == 2, let bytes = uuidBytes(tab), bytes.contains(where: { $0 != 0 }) else { return nil }
    return tab.lowercased()
  }

  public static func threadIdentifier(desktop: String, workspace: String) -> String? {
    guard let d = uuidBytes(desktop), let w = uuidBytes(workspace) else { return nil }
    return hex(Data(SHA256.hash(data: d + w)))
  }

  /// Decide what to show. `open` is `DragentE2EEPushStore.open` in the
  /// extension (reads the shared keychain group); it throws on a locked
  /// keychain, an unknown sender, a bad tag, a mismatched desktop id or a
  /// stale push.
  public static func handle(
    userInfo: [AnyHashable: Any],
    nowMs: UInt64,
    placeholder: DragentPushPlaceholder,
    seen: DragentSeenIds,
    open: (Data, UInt64) throws -> PushNotification
  ) -> DragentPushOutcome {
    guard let text = userInfo[blobKey] as? String else { return .placeholder(placeholder, .missingBlob) }
    guard let sealed = decodeBase64Url(text), sealed.count == Int(pushSealedLen()) else {
      return .placeholder(placeholder, .badEncoding)
    }
    let n: PushNotification
    do {
      n = try open(sealed, nowMs)
    } catch {
      return .placeholder(placeholder, .openFailed)
    }
    if n.dismiss { return .placeholder(placeholder, .dismissal) }
    let id = hex(n.id)
    guard seen.insert(id: id, issuedAtMs: n.issuedAtMs, nowMs: nowMs) else {
      return .placeholder(placeholder, .duplicate)
    }
    // An empty title can't happen from a desktop, but never show a blank alert.
    let title = n.title.isEmpty ? placeholder.title : n.title
    return DragentPushOutcome(
      title: title,
      body: n.body,
      threadIdentifier: threadIdentifier(desktop: n.desktopId, workspace: n.workspaceId),
      notificationId: id,
      desktopId: n.desktopId,
      failure: nil,
      answerPane: answerPane(kind: n.kind, tab: n.tabId)
    )
  }
}
