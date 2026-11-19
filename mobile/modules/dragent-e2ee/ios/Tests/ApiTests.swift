// The generated Swift binding, exercised on the simulator's architecture.
import XCTest
@testable import DragentE2EECore

final class ApiTests: XCTestCase {
  let account = "11111111-2222-4333-8444-555555555555"
  let phone = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"
  let desktop = "01234567-89ab-4cde-8f01-23456789abcd"

  func testKeypairsAndRandomness() throws {
    let a = try Keypair.generate()
    XCTAssertEqual(a.publicKey().count, 32)
    XCTAssertEqual(try Keypair.fromSecret(secret: a.secretBytes()).publicKey(), a.publicKey())
    XCTAssertNotEqual(try Keypair.generate().publicKey(), a.publicKey())
    XCTAssertThrowsError(try Keypair.fromSecret(secret: Data(count: 31)))
    XCTAssertEqual(try randomChannelId().count, 16)
    XCTAssertEqual(sha256(data: Data("abc".utf8)).map { String(format: "%02x", $0) }.joined(),
                   "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
  }

  func testKkStartAndFrames() throws {
    let me = try Keypair.generate()
    let them = try Keypair.generate()
    let chan = try randomChannelId()
    let start = try startKk(identity: me, desktopPin: them.publicKey(),
                            ids: ChannelIds(account: account, phone: phone, desktop: desktop, chan: chan), caps: ["term"])
    XCTAssertGreaterThan(start.hs1.count, 64)
    let frame = try encodeHs1Frame(purpose: .companion, chan: chan, peer: desktop, body: start.hs1)
    let f = try decodeFrame(bytes: frame)
    XCTAssertEqual(f.kind, .hs)
    XCTAssertEqual(f.peer, desktop)
    XCTAssertEqual(f.body, start.hs1)
    // A forged HS2 fails and consumes the handshake.
    XCTAssertThrowsError(try start.handshake.finish(hs2: Data(count: 48), nowMs: 0))
    XCTAssertThrowsError(try start.handshake.finish(hs2: Data(count: 48), nowMs: 0)) { e in
      XCTAssertEqual(errorCode(error: e as! E2eeError), "E2EE_CONSUMED")
    }
  }

  func testRecordsAndReassembly() throws {
    let content = Data((0..<100_000).map { UInt8(truncatingIfNeeded: $0) })
    let recs = try encodeRecords(mtype: .json, msgId: 1, content: content)
    XCTAssertEqual(recs.count, 2)
    let r = Reassembler()
    XCTAssertNil(try r.feed(recordBytes: recs[0]))
    let m = try XCTUnwrap(try r.feed(recordBytes: recs[1]))
    XCTAssertEqual(m.mtype, .json)
    XCTAssertEqual(m.content, content)
    XCTAssertThrowsError(try r.feed(recordBytes: Data(repeating: 0xff, count: 64)))
  }

  func testPinStoreCommitReopenAndLoss() throws {
    let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    let path = dir.appendingPathComponent("state.v1").path
    let key = try generateStateKey()
    let s = try openPinStore(path: path, stateKey: key, keychainGeneration: 0).store
    try s.pinDesktop(device: desktop, identityKey: Data(repeating: 7, count: 32), name: "Mac", catalogVersion: 3, nowMs: 1)
    let g = try s.commit()
    XCTAssertEqual(g, 1)
    let again = try openPinStore(path: path, stateKey: key, keychainGeneration: g)
    XCTAssertEqual(again.store.desktops().map { $0.name }, ["Mac"])
    XCTAssertEqual(try again.store.catalogHighWater(desktop: desktop), 2)
    XCTAssertThrowsError(try openPinStore(path: path, stateKey: try generateStateKey(), keychainGeneration: g)) { e in
      XCTAssertEqual(errorCode(error: e as! E2eeError), "E2EE_STORE_LOST")
    }
    try FileManager.default.removeItem(at: dir)
  }

  func testPushPinsFormat() throws {
    let pins = [PushPin(desktop: desktop, key: Data(repeating: 3, count: 32))]
    let bytes = try encodePushPins(pins: pins)
    XCTAssertEqual(try decodePushPins(bytes: bytes), pins)
    XCTAssertEqual(pushSealedLen(), 1120)
  }
}
