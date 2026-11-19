// §11 / §12.12: native timings on this device, against the phone budget.
import XCTest
@testable import DragentE2EECore

final class BenchmarkTests: XCTestCase {
  func testNativeBudget() throws {
    let results = try runNativeBenchmarks(iterations: 2000)
    for r in results {
      print(String(format: "[e2ee-bench] %@: %.2f µs/op over %u", r.name, r.microsPerOp, r.iterations))
    }
    let by = Dictionary(uniqueKeysWithValues: results.map { ($0.name, $0.microsPerOp) })
    // ≤ 0.2 ms per keystroke round trip; ≤ 1 ms per 64 KiB TERM record.
    XCTAssertLessThan(try XCTUnwrap(by["seal_64b_record"]) + XCTUnwrap(by["open_64b_record"]), 200)
    XCTAssertLessThan(try XCTUnwrap(by["open_64kib_term_record_and_decode"]), 1000)
    // One Swift → Rust call with a 64-byte buffer (the binding's crossing cost).
    let t0 = Date()
    for _ in 0..<2000 { _ = sha256(data: Data(count: 64)) }
    let crossing = Date().timeIntervalSince(t0) * 1e6 / 2000
    print(String(format: "[e2ee-bench] swift_ffi_call_64b: %.2f µs/op", crossing))
  }
}
