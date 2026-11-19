import assert from "node:assert/strict";
import { test } from "node:test";
import { answerAgent, isChoice, isPaneId } from "../src/notifications/answer";

test("only a pane id and allow/deny can be sent as an answer", async () => {
  assert.ok(isPaneId("7c9e6679-7425-40de-944b-e07fc1f90ae7"));
  assert.ok(!isPaneId("7c9e6679742540de944be07fc1f90ae7"));
  assert.ok(!isPaneId("../x"));
  assert.ok(isChoice("allow") && isChoice("deny"));
  assert.ok(!isChoice("always") && !isChoice(""));
  const sent: unknown[] = [];
  const client = { request: async (method: string, params: unknown) => (sent.push([method, params]), { answered: false, reason: "It isn't waiting for an answer any more." }) };
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const result = await answerAgent(client as any, "7C9E6679-7425-40DE-944B-E07FC1F90AE7", "deny");
  assert.deepEqual(sent, [["agent.answer", { pane: "7c9e6679-7425-40de-944b-e07fc1f90ae7", answer: "deny" }]]);
  assert.deepEqual(result, { answered: false, reason: "It isn't waiting for an answer any more." });
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  await assert.rejects(answerAgent(client as any, "nope", "allow"));
});
