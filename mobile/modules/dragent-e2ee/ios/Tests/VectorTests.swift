// §12.1: the crate's Noise and codec vectors, run by the library compiled for
// this simulator (feature `vectors`, debug build only).
import XCTest
@testable import DragentE2EECore

final class VectorTests: XCTestCase {
  func testAllVectorsPass() throws {
    XCTAssertTrue(bindingInfo().vectorsCompiled)
    let root = try XCTUnwrap(Bundle.module.url(forResource: "vectors", withExtension: nil))
    let files = [
      "noise/kk.json", "noise/ikpsk1.json", "noise/x.json",
      "codec/header.json", "codec/record.json", "codec/term.json", "codec/part.json",
      "codec/catalog_blob.json", "codec/push.json", "codec/hs.json",
    ]
    var total: UInt32 = 0
    for f in files {
      let json = try String(contentsOf: root.appendingPathComponent(f), encoding: .utf8)
      do {
        let n = try runVectors(name: f, json: json)
        XCTAssertGreaterThan(n, 0, f)
        print("[e2ee-test] vectors \(f): \(n) passed")
        total += n
      } catch {
        XCTFail("\(f): \(error)")
      }
    }
    print("[e2ee-test] vectors total: \(total) passed")
    // A corrupted vector must fail (the runner really checks bytes).
    var kk = try String(contentsOf: root.appendingPathComponent("noise/kk.json"), encoding: .utf8)
    kk = kk.replacingOccurrences(of: "\"ciphertext\": \"ca35", with: "\"ciphertext\": \"cb35")
    XCTAssertThrowsError(try runVectors(name: "noise/kk.json", json: kk))
  }
}
