import assert from "node:assert/strict";
import { test } from "node:test";

import { goalWithLinks, issueRef, type LinkedIssue } from "./goal";

const issue: LinkedIssue = {
  ref: issueRef("linear", "abc"),
  provider: "linear",
  trackerLabel: "Linear",
  key: "ENG-12",
  title: "Fix sign-in",
  url: "https://linear.app/x/issue/ENG-12",
};

test("the goal is what was typed when nothing is linked", () => {
  assert.equal(goalWithLinks("  Ship it ", [], []), "Ship it");
});

test("linked issues follow the goal as the desktop links them", () => {
  assert.equal(
    goalWithLinks("Ship it", [issue], []),
    "Ship it\n\n## Linked issue — ENG-12: Fix sign-in\nhttps://linear.app/x/issue/ENG-12",
  );
});

test("an issue alone is a goal; files come last", () => {
  const goal = goalWithLinks("", [{ ...issue, url: null }], ["/tmp/a b.png"]);
  assert.equal(
    goal,
    "Work on the linked issues below. Use their details as context.\n\n## Linked issue — ENG-12: Fix sign-in\n\n'/tmp/a b.png'",
  );
});
