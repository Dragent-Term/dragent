import assert from "node:assert/strict";
import { test } from "node:test";
import { ageLabel, bytesLabel, groupByPeriod, matchesSearch, periodOf, readPastSessions } from "../src/history/model";

const now = new Date(2026, 10, 16, 15, 0, 0);
const at = (day: number, hour = 9) => new Date(2026, 10, day, hour).getTime() / 1000;

test("the desktop's list is read, and unreadable rows are left out", () => {
  const sessions = readPastSessions({
    sessions: [
      { provider: "claude", id: "a", title: "Fix login", updated_at: 100, account_label: "Work", size: 2048 },
      { provider: "opencode", id: "ses_b", title: "", updated_at: 50, account_label: null, size: null },
      { provider: 3, id: "x", title: "bad" },
    ],
  });
  assert.ok(sessions);
  assert.deepEqual(sessions.map((s) => [s.provider, s.title, s.accountLabel, s.size]), [
    ["claude", "Fix login", "Work", 2048],
    ["opencode", "Untitled", null, null],
  ]);
  assert.equal(readPastSessions({}), null);
});

test("conversations sit under the day they last changed, newest first", () => {
  assert.equal(periodOf(at(16, 1), now), "TODAY");
  assert.equal(periodOf(at(15, 23), now), "YESTERDAY");
  assert.equal(periodOf(at(11), now), "THIS WEEK");
  assert.equal(periodOf(at(2), now), "THIS MONTH");
  assert.equal(periodOf(new Date(2026, 6, 1).getTime() / 1000, now), "EARLIER");
  const base = { provider: "claude", accountLabel: null, size: null };
  const groups = groupByPeriod(
    [
      { ...base, id: "old", title: "old", updatedAt: at(2) },
      { ...base, id: "new", title: "new", updatedAt: at(16) },
      { ...base, id: "newer", title: "newer", updatedAt: at(16, 12) },
    ],
    now,
  );
  assert.deepEqual(groups.map((g) => [g.caption, g.sessions.map((s) => s.id)]), [
    ["TODAY", ["newer", "new"]],
    ["THIS MONTH", ["old"]],
  ]);
});

test("ages, sizes and search read naturally", () => {
  const nowMs = 10_000_000_000;
  assert.equal(ageLabel(nowMs / 1000 - 30, nowMs), "just now");
  assert.equal(ageLabel(nowMs / 1000 - 120, nowMs), "2 minutes ago");
  assert.equal(ageLabel(nowMs / 1000 - 3600, nowMs), "1 hour ago");
  assert.equal(bytesLabel(1.5 * 1024 ** 3), "1.5 GB");
  assert.equal(bytesLabel(840 * 1024 ** 2), "840 MB");
  assert.equal(bytesLabel(10), "1 KB");
  const session = { provider: "opencode", id: "x", title: "Fix the login redirect", updatedAt: 0, accountLabel: null, size: null };
  assert.ok(matchesSearch(session, "OpenCode", "login fix"));
  assert.ok(matchesSearch(session, "OpenCode", "opencode"));
  assert.ok(!matchesSearch(session, "OpenCode", "login codex"));
});
