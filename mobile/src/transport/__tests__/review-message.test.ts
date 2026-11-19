// A review written on the phone reaches the agent as one clear message.
import assert from "node:assert/strict";
import { test } from "node:test";

import { addReviewComment, clearReview, reviewComments, reviewMessage, removeReviewComment } from "../../machines/review";

test("comments collect per workspace and read as one message grouped by file", () => {
  const root = "/work/app";
  clearReview(root);
  addReviewComment(root, { file: "src/a.ts", line: 3, side: "new", quote: "const x = 1;", text: "Name this better" });
  addReviewComment(root, { file: "src/b.ts", line: 9, side: "old", quote: "oldThing()", text: "Why remove this?" });
  addReviewComment(root, { file: "src/a.ts", line: 7, side: "new", quote: "", text: "Add a test" });
  assert.equal(reviewComments(root).length, 3);
  assert.equal(reviewComments("/other").length, 0);
  const message = reviewMessage(reviewComments(root), "Then run the tests.");
  assert.equal(message, [
    "Please address these review comments on the current changes:",
    "",
    "src/a.ts:",
    "- line 3: Name this better — `const x = 1;`",
    "- line 7: Add a test",
    "",
    "src/b.ts:",
    "- line 9 (removed): Why remove this? — `oldThing()`",
    "",
    "Then run the tests.",
  ].join("\n"));
  removeReviewComment(root, reviewComments(root)[0]!.id);
  assert.equal(reviewComments(root).length, 2);
  clearReview(root);
  assert.equal(reviewComments(root).length, 0);
});
