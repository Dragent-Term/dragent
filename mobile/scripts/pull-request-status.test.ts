import assert from "node:assert/strict";
import { test } from "node:test";
import { readPullRequest, readiness, sortChecks } from "../src/pullRequests/model";

const raw = {
  pr: {
    number: 42, title: "Fix login", url: "https://github.com/o/r/pull/42", state: "open", draft: false,
    review: "review_required", mergeable: "mergeable", head: "fix-login", base: "main",
    checks: [
      { name: "test", workflow: "CI", state: "passed", url: "https://ci/1" },
      { name: "lint", workflow: "CI", state: "failed", url: "javascript:alert(1)" },
      { name: "build", workflow: "CI", state: "running", url: "" },
      { name: "", state: "passed" },
      { name: "odd", state: "weird" },
    ],
  },
};

test("a pull request and its checks are read; bad links and nameless checks are dropped", () => {
  const reading = readPullRequest(raw);
  assert.ok(reading && reading.pr);
  assert.equal(reading.pr.number, 42);
  assert.deepEqual(reading.pr.checks.map((c) => [c.name, c.state, c.url]), [
    ["test", "passed", "https://ci/1"],
    ["lint", "failed", null],
    ["build", "running", null],
    ["odd", "neutral", null],
  ]);
  assert.deepEqual(sortChecks(reading.pr.checks).map((c) => c.name), ["lint", "build", "test", "odd"]);
  assert.deepEqual(readPullRequest({ pr: null, reason: "This branch has no pull request yet." }), { pr: null, reason: "This branch has no pull request yet." });
  assert.equal(readPullRequest({ pr: { title: "no number" } }), null);
});

test("its standing says what's in the way first", () => {
  const pr = readPullRequest(raw)!.pr!;
  assert.deepEqual(readiness(pr), { label: "1 check failing", tone: "danger" });
  assert.equal(readiness({ ...pr, mergeable: "conflicting" }).label, "Conflicts with the base branch");
  assert.equal(readiness({ ...pr, checks: [] }).label, "Waiting for review");
  assert.equal(readiness({ ...pr, checks: [], review: "approved" }).label, "Ready to merge");
  assert.equal(readiness({ ...pr, state: "merged" }).label, "Merged");
});
