// Simulator test host: runs the XCTest suite inside an app process that is
// signed with keychain entitlements (an xctest agent process has none, so
// keychain calls fail with errSecMissingEntitlement there).
import Foundation
import XCTest

let suite = XCTestSuite.default
suite.run()
let run = suite.testRun!
print("[e2ee-test] RESULT executed=\(run.executionCount) failures=\(run.totalFailureCount)")
fflush(stdout)
exit(run.totalFailureCount == 0 && run.executionCount > 0 ? 0 : 1)
