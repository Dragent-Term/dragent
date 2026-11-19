// The phone's end-to-end encryption module (docs/design/e2ee.md §3.2, E4).
//
// JS reaches `dragent_e2ee` (Rust, through uniffi) over Expo's JSI module
// bridge. Private keys never cross into JS: the identity and push key pairs
// are Rust objects held here, loaded from the keychain with the §6.7 classes
// (DragentE2EEKeychain.swift). JS gets public keys and opaque handshake,
// channel, reassembler, part and pin-store objects.

import ExpoModulesCore
import Foundation
import LocalAuthentication

// MARK: - Shared object wrappers

final class ChannelRef: SharedRef<Channel> {}
final class KkHandshakeRef: SharedRef<KkHandshake> {
  let hs1: Data
  var caps: [String] = []
  init(_ ref: KkHandshake, hs1: Data) {
    self.hs1 = hs1
    super.init(ref)
  }
}
final class PairingHandshakeRef: SharedRef<PairingHandshake> {
  let hs1: Data
  init(_ ref: PairingHandshake, hs1: Data) {
    self.hs1 = hs1
    super.init(ref)
  }
}
final class ReassemblerRef: SharedRef<Reassembler> {}
final class PartSessionRef: SharedRef<PartSession> {}
final class PinStoreRef: SharedRef<PinStore> {}

// MARK: - Errors

func e2eeException(_ error: Error) -> Exception {
  if let e = error as? E2eeError {
    return Exception(name: "E2EEError", description: "\(e)", code: errorCode(error: e))
  }
  if let e = error as? DragentE2EEKeychainError {
    switch e {
    case .noPasscode:
      return Exception(name: "E2EEError", description: "Set a passcode on this phone to pair it.", code: "E2EE_NO_PASSCODE")
    case .locked:
      return Exception(name: "E2EEError", description: "The phone is locked.", code: "E2EE_LOCKED")
    case .notConfigured(let what):
      return Exception(name: "E2EEError", description: "Keychain access group not configured: \(what)", code: "E2EE_NOT_CONFIGURED")
    case .status(let s):
      return Exception(name: "E2EEError", description: "Keychain error \(s)", code: "E2EE_KEYCHAIN")
    }
  }
  return Exception(name: "E2EEError", description: "\(error)", code: "E2EE_INTERNAL")
}

/// Runs `body`, turning Rust and keychain errors into coded JS exceptions.
func e2ee<T>(_ body: () throws -> T) throws -> T {
  do {
    return try body()
  } catch let e as Exception {
    throw e
  } catch {
    throw e2eeException(error)
  }
}

func nullable(_ v: Any?) -> Any { v ?? NSNull() }

// MARK: - Record conversions (Rust records → JS objects)

func frameKindName(_ k: FrameKind) -> String {
  switch k { case .hs: return "hs"; case .data: return "data"; case .close: return "close" }
}

func framePurposeName(_ p: FramePurpose?) -> Any {
  switch p { case .companion: return "companion"; case .pairing: return "pairing"; case nil: return NSNull() }
}

func parseFramePurpose(_ s: String?) throws -> FramePurpose? {
  switch s {
  case nil: return nil
  case "companion": return .companion
  case "pairing": return .pairing
  default: throw Exception(name: "E2EEError", description: "unknown purpose", code: "E2EE_INVALID_ARGUMENT")
  }
}

func parseFrameKind(_ s: String) throws -> FrameKind {
  switch s {
  case "hs": return .hs
  case "data": return .data
  case "close": return .close
  default: throw Exception(name: "E2EEError", description: "unknown frame kind", code: "E2EE_INVALID_ARGUMENT")
  }
}

func messageTypeName(_ m: MessageType) -> String {
  switch m { case .json: return "json"; case .term: return "term"; case .ctrl: return "ctrl" }
}

func parseMessageType(_ s: String) throws -> MessageType {
  switch s {
  case "json": return .json
  case "term": return .term
  case "ctrl": return .ctrl
  default: throw Exception(name: "E2EEError", description: "unknown message type", code: "E2EE_INVALID_ARGUMENT")
  }
}

func stateObject(_ s: ChannelStateKind) -> [String: Any] {
  switch s {
  case .awaitingConfirm: return ["state": "awaiting_confirm"]
  case .confirming: return ["state": "confirming"]
  case .open: return ["state": "open"]
  case .windingDown: return ["state": "winding_down"]
  case .closed(let reason): return ["state": "closed", "reason": reason]
  }
}

func parseCloseReason(_ s: String) throws -> LocalCloseReason {
  switch s {
  case "local": return .local
  case "peer_closed": return .peerClosed
  case "revoked": return .revoked
  case "rekeyed": return .rekeyed
  default: throw Exception(name: "E2EEError", description: "unknown close reason", code: "E2EE_INVALID_ARGUMENT")
  }
}

func pushObject(_ n: PushNotification) -> [String: Any] {
  [
    "desktopId": n.desktopId, "id": n.id, "issuedAtMs": Double(n.issuedAtMs), "kind": Int(n.kind),
    "dismiss": n.dismiss, "title": n.title, "body": n.body, "workspaceId": n.workspaceId,
    "tabId": n.tabId, "dismissIds": n.dismissIds,
  ]
}

struct ChannelIdsArg: Record {
  @Field var account: String = ""
  @Field var phone: String = ""
  @Field var desktop: String = ""
  @Field var chan: Data = Data()

  var rust: ChannelIds { ChannelIds(account: account, phone: phone, desktop: desktop, chan: chan) }
}

struct FrameArg: Record {
  @Field var kind: String = ""
  @Field var step: Int = 0
  @Field var purpose: String? = nil
  @Field var chan: Data = Data()
  @Field var peer: String? = nil
  @Field var ctr: Double = 0
  @Field var body: Data = Data()
}

// MARK: - Module

public final class DragentE2EEModule: Module {
  private let keychain = DragentE2EEKeychain.fromBundle()
  private let lock = NSLock()
  private var identityCache: Keypair?

  /// The identity key pair, read from the keychain once per process.
  private func identity() throws -> Keypair {
    lock.lock()
    defer { lock.unlock() }
    if let k = identityCache { return k }
    guard var secret = try keychain.read(.identity) else {
      throw Exception(name: "E2EEError", description: "No identity key on this phone; pair again.", code: "E2EE_NO_IDENTITY")
    }
    defer { secret.resetBytes(in: 0..<secret.count) }
    let k = try Keypair.fromSecret(secret: secret)
    identityCache = k
    return k
  }

  private func store(_ item: DragentE2EEKeychainItem, keypair k: Keypair) throws {
    var secret = k.secretBytes()
    defer { secret.resetBytes(in: 0..<secret.count) }
    try keychain.write(item, secret)
  }

  private func pushPublic() throws -> Data? {
    guard var secret = try keychain.read(.push) else { return nil }
    defer { secret.resetBytes(in: 0..<secret.count) }
    return try Keypair.fromSecret(secret: secret).publicKey()
  }

  private func createPush() throws -> Data {
    if let existing = try pushPublic() { return existing }
    let k = try Keypair.generate()
    try store(.push, keypair: k)
    return k.publicKey()
  }

  private func pinStorePath() throws -> String {
    let docs = try FileManager.default.url(for: .documentDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
    let dir = docs.appendingPathComponent("e2ee", isDirectory: true)
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    var url = dir.appendingPathComponent("state.v1")
    // The store is useless without this device's keychain; keep it out of backups.
    var values = URLResourceValues()
    values.isExcludedFromBackup = true
    try? url.setResourceValues(values)
    return url.path
  }

  private func readGeneration() throws -> UInt64 {
    guard let d = try keychain.read(.pinStoreGeneration) else { return 0 }
    guard let g = DragentE2EEPushStore.readU64(d) else {
      throw E2eeError.StoreLost(reason: "generation")
    }
    return g
  }

  private func writeGeneration(_ g: UInt64) throws {
    try keychain.write(.pinStoreGeneration, DragentE2EEPushStore.u64(g))
  }

  private func openStore() throws -> PinStore {
    let gen = try readGeneration()
    var key: Data
    if let k = try keychain.read(.pinStoreKey) {
      key = k
    } else {
      // No state key: only a brand-new store (generation 0, no file) may start fresh.
      guard gen == 0 else { throw E2eeError.StoreLost(reason: "missing state key") }
      key = try generateStateKey()
      try keychain.write(.pinStoreKey, key)
    }
    defer { key.resetBytes(in: 0..<key.count) }
    let opened = try openPinStore(path: try pinStorePath(), stateKey: key, keychainGeneration: gen)
    if let bump = opened.bumpKeychainTo {
      try writeGeneration(bump)
    }
    return opened.store
  }

  private func resetStore() throws {
    let path = try pinStorePath()
    if FileManager.default.fileExists(atPath: path) {
      try FileManager.default.removeItem(atPath: path)
    }
    try keychain.delete(.pinStoreKey)
    try keychain.delete(.pinStoreGeneration)
  }

  private func readPins() throws -> [PushPin] {
    try DragentE2EEPushStore.pins(keychain)
  }

  private func writePins(_ pins: [PushPin]) throws {
    try keychain.write(.pushPins, try encodePushPins(pins: pins))
  }

  // swiftlint:disable:next function_body_length cyclomatic_complexity
  public func definition() -> ModuleDefinition {
    Name("DragentE2EE")

    Constant("platform") { "ios" }
    Constant("vectorsCompiled") { bindingInfo().vectorsCompiled }

    // MARK: Device and keys (§6.1, §6.7)

    Function("hasDevicePasscode") { () -> Bool in
      LAContext().canEvaluatePolicy(.deviceOwnerAuthentication, error: nil)
    }

    Function("identityPublicKey") { () -> Data? in
      try e2ee {
        guard try self.keychain.read(.identity) != nil else { return nil }
        return try self.identity().publicKey()
      }
    }

    /// §6.2 step 4: refused without a passcode (the item class needs one).
    Function("createIdentityKey") { () -> Data in
      try e2ee {
        if try self.keychain.read(.identity) != nil { return try self.identity().publicKey() }
        guard LAContext().canEvaluatePolicy(.deviceOwnerAuthentication, error: nil) else {
          throw DragentE2EEKeychainError.noPasscode
        }
        let k = try Keypair.generate()
        try self.store(.identity, keypair: k)
        self.lock.lock()
        self.identityCache = k
        self.lock.unlock()
        return k.publicKey()
      }
    }

    Function("pushPublicKey") { () -> Data? in try e2ee { try self.pushPublic() } }

    Function("createPushKey") { () -> Data in try e2ee { try self.createPush() } }

    /// §6.6: a new push key; the old one opens notifications for 7 more days.
    Function("rotatePushKey") { (nowMs: Double) -> Data in
      try e2ee {
        if var old = try self.keychain.read(.push) {
          defer { old.resetBytes(in: 0..<old.count) }
          var prev = DragentE2EEPushStore.u64(UInt64(nowMs))
          prev.append(old)
          defer { prev.resetBytes(in: 0..<prev.count) }
          try self.keychain.write(.pushPrevious, prev)
        }
        let k = try Keypair.generate()
        try self.store(.push, keypair: k)
        return k.publicKey()
      }
    }

    /// §6.4 step 5: the last desktop is gone, so every key and record goes.
    Function("deleteAllKeys") {
      try e2ee {
        self.lock.lock()
        self.identityCache = nil
        self.lock.unlock()
        try self.resetStore()
        for item in DragentE2EEKeychainItem.all {
          try self.keychain.delete(item)
        }
      }
    }

    /// The stored class of each item, read back from the keychain.
    Function("keyStorageInfo") { () -> [[String: Any]] in
      try e2ee {
        try DragentE2EEKeychainItem.all.compactMap { item in
          guard let a = try self.keychain.storedAttributes(item) else { return nil }
          return [
            "item": item.account, "keyClass": a.keyClass.rawValue,
            "accessGroup": nullable(a.accessGroup), "synchronizable": a.synchronizable,
          ]
        }
      }
    }

    // MARK: Push pins and push (§8)

    Function("setPushContext") { (account: String, phone: String) in
      try e2ee {
        let d = try JSONSerialization.data(withJSONObject: ["account": account, "phone": phone], options: [.sortedKeys])
        try self.keychain.write(.pushContext, d)
      }
    }

    Function("listPushPins") { () -> [[String: Any]] in
      try e2ee { try self.readPins().map { ["desktop": $0.desktop, "key": $0.key] } }
    }

    Function("setPushPin") { (desktop: String, key: Data) in
      try e2ee {
        var pins = try self.readPins().filter { $0.desktop != desktop && $0.key != key }
        pins.append(PushPin(desktop: desktop, key: key))
        try self.writePins(pins)
      }
    }

    Function("removePushPin") { (desktop: String) in
      try e2ee { try self.writePins(try self.readPins().filter { $0.desktop != desktop }) }
    }

    Function("openPush") { (sealed: Data, nowMs: Double) -> [String: Any] in
      try e2ee { pushObject(try DragentE2EEPushStore.open(sealed: sealed, nowMs: UInt64(nowMs), keychain: self.keychain)) }
    }

    // MARK: Pin store (§6.1)

    Function("openPinStore") { () -> PinStoreRef in try e2ee { PinStoreRef(try self.openStore()) } }

    Function("resetPinStore") { try e2ee { try self.resetStore() } }

    Class(PinStoreRef.self) {
      Function("generation") { (s: PinStoreRef) -> Double in Double(s.ref.generation()) }
      Function("desktops") { (s: PinStoreRef) -> [[String: Any]] in
        s.ref.desktops().map {
          [
            "id": $0.id, "identityKey": $0.identityKey, "name": $0.name,
            "pinnedAtMs": Double($0.pinnedAtMs), "rotationKey": nullable($0.rotationKey),
          ]
        }
      }
      Function("lookup") { (s: PinStoreRef, device: String) -> [String: Any] in
        try e2ee {
          switch try s.ref.lookup(device: device) {
          case .pinned(let key): return ["state": "pinned", "identityKey": key]
          case .unpinned: return ["state": "unpinned"]
          case .tombstoned: return ["state": "tombstoned"]
          }
        }
      }
      Function("candidateKeys") { (s: PinStoreRef, device: String, nowMs: Double) -> [Data] in
        try e2ee { try s.ref.candidateKeys(device: device, nowMs: UInt64(nowMs)) }
      }
      Function("pinDesktop") { (s: PinStoreRef, device: String, key: Data, name: String, catalogVersion: Double, nowMs: Double) in
        try e2ee {
          try s.ref.pinDesktop(device: device, identityKey: key, name: name, catalogVersion: UInt64(catalogVersion), nowMs: UInt64(nowMs))
        }
      }
      Function("announceRotation") { (s: PinStoreRef, device: String, key: Data, nowMs: Double) in
        try e2ee { try s.ref.announceRotation(device: device, newKey: key, nowMs: UInt64(nowMs)) }
      }
      Function("confirmRotation") { (s: PinStoreRef, device: String, key: Data) -> Bool in
        try e2ee { try s.ref.confirmRotation(device: device, confirmedKey: key) }
      }
      Function("revoke") { (s: PinStoreRef, device: String, nowMs: Double) -> Bool in
        try e2ee { try s.ref.revoke(device: device, nowMs: UInt64(nowMs)) }
      }
      Function("openCatalog") { (s: PinStoreRef, account: String, phone: String, desktop: String, blob: Data) -> [String: Any] in
        try e2ee {
          switch try s.ref.openCatalog(identity: try self.identity(), account: account, phone: phone, desktop: desktop, blob: blob) {
          case .accepted(let version, let json): return ["status": "accepted", "version": Double(version), "json": json]
          case .unchanged(let version): return ["status": "unchanged", "version": Double(version)]
          }
        }
      }
      Function("catalogHighWater") { (s: PinStoreRef, desktop: String) -> Double? in
        try e2ee { try s.ref.catalogHighWater(desktop: desktop).map(Double.init) }
      }
      /// Persist, then record the new generation in the keychain.
      Function("commit") { (s: PinStoreRef) -> Double in
        try e2ee {
          let g = try s.ref.commit()
          try self.writeGeneration(g)
          return Double(g)
        }
      }
    }

    // MARK: Handshakes and channels (§4.2, §6.2)

    Function("startPairing") { (ids: ChannelIdsArg, desktopKey: Data, secretP: Data, deviceName: String) -> PairingHandshakeRef in
      var p = secretP
      defer { p.resetBytes(in: 0..<p.count) }
      return try e2ee {
        let start = try startPairing(
          identity: try self.identity(), pushPublicKey: try self.createPush(), desktopKeyFromQr: desktopKey,
          secretP: p, ids: ids.rust, deviceName: deviceName, platform: .ios
        )
        return PairingHandshakeRef(start.handshake, hs1: start.hs1)
      }
    }

    Class(PairingHandshakeRef.self) {
      Property("hs1") { (h: PairingHandshakeRef) -> Data in h.hs1 }
      Function("finish") { (h: PairingHandshakeRef, hs2: Data, nowMs: Double) -> ChannelRef in
        try e2ee { ChannelRef(try h.ref.finish(hs2: hs2, nowMs: UInt64(nowMs))) }
      }
      Function("abandon") { (h: PairingHandshakeRef) in h.ref.abandon() }
    }

    Function("startKk") { (ids: ChannelIdsArg, desktopPin: Data, caps: [String]) -> KkHandshakeRef in
      try e2ee {
        let start = try startKk(identity: try self.identity(), desktopPin: desktopPin, ids: ids.rust, caps: caps)
        return KkHandshakeRef(start.handshake, hs1: start.hs1)
      }
    }

    Class(KkHandshakeRef.self) {
      Property("hs1") { (h: KkHandshakeRef) -> Data in h.hs1 }
      Property("peerCaps") { (h: KkHandshakeRef) -> [String] in h.caps }
      Function("finish") { (h: KkHandshakeRef, hs2: Data, nowMs: Double) -> ChannelRef in
        try e2ee {
          let done = try h.ref.finish(hs2: hs2, nowMs: UInt64(nowMs))
          h.caps = done.caps
          return ChannelRef(done.channel)
        }
      }
      Function("abandon") { (h: KkHandshakeRef) in h.ref.abandon() }
    }

    Class(ChannelRef.self) {
      Property("chanId") { (c: ChannelRef) -> Data in c.ref.chanId() }
      Function("state") { (c: ChannelRef) -> [String: Any] in stateObject(c.ref.state()) }
      Function("handshakeHash") { (c: ChannelRef) -> Data in c.ref.handshakeHash() }
      Function("remoteStatic") { (c: ChannelRef) -> Data in c.ref.remoteStatic() }
      Function("hasKeys") { (c: ChannelRef) -> Bool in c.ref.hasKeys() }
      Function("processedThrough") { (c: ChannelRef) -> Double? in c.ref.processedThrough().map(Double.init) }
      Function("lastSentCtr") { (c: ChannelRef) -> Double? in c.ref.lastSentCtr().map(Double.init) }
      Function("isQuiesced") { (c: ChannelRef) -> Bool in c.ref.isQuiesced() }
      Function("seal") { (c: ChannelRef, record: Data) -> [String: Any] in
        try e2ee {
          let s = try c.ref.seal(record: record)
          return ["ctr": Double(s.ctr), "ciphertext": s.ciphertext]
        }
      }
      Function("sealFrame") { (c: ChannelRef, record: Data) -> Data in try e2ee { try c.ref.sealFrame(record: record) } }
      Function("receive") { (c: ChannelRef, ctr: Double, ciphertext: Data) -> [String: Any] in
        try e2ee {
          switch try c.ref.receive(ctr: UInt64(ctr), ciphertext: ciphertext) {
          case .record(let r): return ["kind": "record", "record": r]
          case .confirming(let r): return ["kind": "confirming", "record": r]
          case .duplicate: return ["kind": "duplicate"]
          }
        }
      }
      Function("quiesce") { (c: ChannelRef) -> [String: Any] in
        let r = c.ref.quiesce()
        return ["chan": r.chan, "lastSentCtr": nullable(r.lastSentCtr.map(Double.init))]
      }
      Function("poll") { (c: ChannelRef, nowMs: Double) -> String? in c.ref.poll(nowMs: UInt64(nowMs)) }
      Function("rekeyDue") { (c: ChannelRef, nowMs: Double) -> Bool in c.ref.rekeyDue(nowMs: UInt64(nowMs)) }
      Function("close") { (c: ChannelRef, reason: String) in
        try e2ee { c.ref.close(reason: try parseCloseReason(reason)) }
      }
    }

    // MARK: Codecs (§4.1, §4.4, §4.5)

    Function("randomChannelId") { () -> Data in try e2ee { try randomChannelId() } }
    Function("randomBytes") { (n: Int) -> Data in try e2ee { try randomBytes(n: UInt32(clamping: n)) } }
    Function("sha256") { (data: Data) -> Data in sha256(data: data) }

    Function("decodeFrame") { (bytes: Data) -> [String: Any] in
      try e2ee {
        let f = try decodeFrame(bytes: bytes)
        return [
          "kind": frameKindName(f.kind), "step": Int(f.step), "purpose": framePurposeName(f.purpose),
          "chan": f.chan, "peer": nullable(f.peer), "ctr": Double(f.ctr), "body": f.body,
        ]
      }
    }

    Function("encodeFrame") { (f: FrameArg) -> Data in
      try e2ee {
        try encodeFrame(frame: Frame(
          kind: try parseFrameKind(f.kind), step: UInt8(clamping: f.step), purpose: try parseFramePurpose(f.purpose),
          chan: f.chan, peer: f.peer, ctr: UInt64(f.ctr), body: f.body
        ))
      }
    }

    Function("encodeHs1Frame") { (purpose: String, chan: Data, peer: String, body: Data) -> Data in
      try e2ee {
        guard let p = try parseFramePurpose(purpose) else { throw E2eeError.InvalidArgument(detail: "purpose") }
        return try encodeHs1Frame(purpose: p, chan: chan, peer: peer, body: body)
      }
    }

    Function("encodeRecords") { (mtype: String, msgId: Double, content: Data) -> [Data] in
      try e2ee { try encodeRecords(mtype: try parseMessageType(mtype), msgId: UInt32(msgId), content: content) }
    }

    Function("encodePadRecord") { (len: Int) -> Data in try e2ee { try encodePadRecord(len: UInt32(clamping: len)) } }

    Function("decodeTerm") { (content: Data) -> [String: Any] in
      try e2ee {
        let t = try decodeTerm(content: content)
        return [
          "handle": Double(t.handle), "seq": Double(t.seq), "base": Double(t.base), "total": Double(t.total),
          "alive": t.alive, "reset": t.reset, "exitCode": nullable(t.exitCode.map(Int.init)),
          "rows": nullable(t.rows.map(Int.init)), "cols": nullable(t.cols.map(Int.init)), "data": t.data,
        ]
      }
    }

    Function("decodeCtrl") { (content: Data) -> [String: Any] in
      try e2ee {
        let c = try decodeCtrl(content: content)
        return ["name": c.name, "json": c.json]
      }
    }

    Function("encodeCtrl") { (name: String, json: String) -> Data in try e2ee { try encodeCtrl(name: name, json: json) } }

    Function("createReassembler") { () -> ReassemblerRef in ReassemblerRef(Reassembler()) }

    Class(ReassemblerRef.self) {
      Function("feed") { (r: ReassemblerRef, record: Data) -> [String: Any]? in
        try e2ee {
          guard let m = try r.ref.feed(recordBytes: record) else { return nil }
          return ["mtype": messageTypeName(m.mtype), "content": m.content]
        }
      }
      Function("buffered") { (r: ReassemblerRef) -> Double in Double(r.ref.buffered()) }
      Function("clear") { (r: ReassemblerRef) in r.ref.clear() }
    }

    Function("beginPart") { (ctrlContent: Data) -> PartSessionRef in
      try e2ee { PartSessionRef(try PartSession.begin(ctrlContent: ctrlContent)) }
    }

    Class(PartSessionRef.self) {
      Function("info") { (p: PartSessionRef) -> [String: Any] in
        let i = p.ref.info()
        let purpose: String
        switch i.purpose { case .seed: purpose = "seed"; case .screen: purpose = "screen"; case .reply: purpose = "reply" }
        return [
          "partId": i.partId, "purpose": purpose, "requestIdJson": nullable(i.requestIdJson),
          "streamHandle": nullable(i.streamHandle.map(Double.init)), "totalBytes": Double(i.totalBytes),
          "partCount": Double(i.partCount),
        ]
      }
      Function("feed") { (p: PartSessionRef, chunkJson: Data) in try e2ee { try p.ref.feed(chunkJson: chunkJson) } }
      Function("end") { (p: PartSessionRef, ctrlContent: Data) -> Data in try e2ee { try p.ref.end(ctrlContent: ctrlContent) } }
      Function("abort") { (p: PartSessionRef) in p.ref.abort() }
    }

    // MARK: Benchmarks (§11, §12.12)

    /// Times the native operations on this device (no JSI in the loop; the
    /// JS-side benchmark adds the crossing).
    Function("nativeBenchmarks") { (iterations: Int) -> [[String: Any]] in
      try e2ee {
        try runNativeBenchmarks(iterations: UInt32(clamping: max(1, iterations))).map {
          ["name": $0.name, "iterations": Double($0.iterations), "microsPerOp": $0.microsPerOp]
        }
      }
    }
  }
}
