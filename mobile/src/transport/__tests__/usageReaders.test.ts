import assert from "node:assert/strict";
import { test } from "node:test";
import { readUsageSummary, readAccountsReply } from "../../usage/readers";
import { formatMeasure, limitTone, resetsIn, shortCount } from "../../usage/types";

const totals = { tokens: 1500, input: 1000, output: 400, cache_read: 100, cache_write: 0, replies: 3, cost: 0.42, unpriced_tokens: 0, cache_savings: 0.01 };
const reply = {
  range: { from: "2026-11-10", to: "2026-11-16", days: 7 },
  total: totals,
  agents: [{ agent: "claude", totals }],
  models: [{ agent: "claude", model: "claude-sonnet-4-5", totals }],
  projects: [{ name: "app", path: "/Users/me/app", totals }],
  days: [{ date: "2026-11-16", agents: { claude: { tokens: 1500, cost: 0.42 } } }],
  accounts: [
    { provider: "claude", account: "system", label: "System login", default: true, plan: "max", updated_at: 10,
      windows: [{ label: "5-hour", used_percent: 42.5, resets_at: 100 }] },
    { provider: "codex", account: "system", label: "System login", default: true, plan: null, updated_at: null, windows: [] },
  ],
};

test("the desktop's usage summary reads", () => {
  const summary = readUsageSummary(reply);
  assert.ok(summary);
  assert.equal(summary.days, 7);
  assert.equal(summary.total.cacheRead, 100);
  assert.equal(summary.accounts[0]!.inUse, true);
  assert.equal(summary.accounts[0]!.windows[0]!.usedPercent, 42.5);
  assert.equal(summary.daily[0]!.agents.claude!.tokens, 1500);
});

test("malformed usage is refused rather than shown wrong", () => {
  assert.equal(readUsageSummary({ ...reply, total: { ...totals, tokens: -1 } }), null);
  assert.equal(readUsageSummary({ ...reply, days: [{ date: "yesterday", agents: {} }] }), null);
  assert.equal(readAccountsReply({ accounts: [{ provider: "claude" }] }), null);
});

test("numbers read the way the desktop page writes them", () => {
  assert.equal(shortCount(1_234_567), "1.2M");
  assert.equal(formatMeasure(12.3, "cost"), "$12.30");
  assert.equal(limitTone(95), "danger");
  assert.equal(limitTone(75), "warning");
  assert.equal(limitTone(10), "success");
  assert.equal(resetsIn(3600 * 2 + 14 * 60 + 30, 0), "in 2 h 14 min");
  assert.equal(resetsIn(null, 0), null);
});
