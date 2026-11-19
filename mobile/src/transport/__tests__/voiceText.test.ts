import assert from "node:assert/strict";
import { test } from "node:test";
import { spokenText } from "../../chat/voice";

test("a reply reads naturally: no markdown, code and links named", () => {
  assert.equal(spokenText("## Done\n\n- Fixed **the bug** in `relay.go`\n- See [the PR](https://x.y/1)"), "Done. Fixed the bug in relay.go See the PR");
  assert.equal(spokenText("Here:\n```ts\nconst a = 1;\n```"), "Here: I've left the code in the chat.");
  assert.match(spokenText("Long. ".repeat(400), 100), /The rest is in the chat\.$/);
  assert.equal(spokenText(""), "Done. It's in the chat.");
});
