import { test } from "node:test";
import assert from "node:assert/strict";
import { appendDictation, dictationFailure, meterLevel, pushLevel, recordingTime } from "../../features/chat/dictationText";

test("dictation adds to the draft, never replaces it", () => {
  assert.equal(appendDictation("", " fix the login bug "), "fix the login bug");
  assert.equal(appendDictation("Please", "fix it"), "Please fix it");
  assert.equal(appendDictation("Line one\n", "two"), "Line one\ntwo");
  assert.equal(appendDictation("Keep", "   "), "Keep");
});

test("the meter keeps the newest five levels", () => {
  let levels: number[] = [];
  for (const volume of [-2, 0, 5, 10, 20, 2.5]) levels = pushLevel(levels, meterLevel(volume));
  assert.deepEqual(levels, [0, 0.5, 1, 1, 0.25]);
});

test("recording time and failures read plainly", () => {
  assert.equal(recordingTime(7_400), "0:07");
  assert.equal(recordingTime(83_000), "1:23");
  assert.equal(dictationFailure("no-speech"), null);
  assert.match(dictationFailure("not-allowed") ?? "", /Settings/);
});
