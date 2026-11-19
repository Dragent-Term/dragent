import assert from "node:assert/strict";
import { test } from "node:test";
import { RelayRpcClient, type RelayRpcError } from "../src/transport/relayRpcClient";

test("subscription rejection reaches its owner and releases the stream", () => {
  const id = "00000000-0000-4000-8000-000000000001";
  const client = new RelayRpcClient({ send: () => true, createRequestId: () => id });
  const errors: RelayRpcError[] = [];
  let snapshots = 0;
  const subscription = client.subscribe("catalog", { device_id: "desktop" }, () => { snapshots++; }, (error) => errors.push(error));
  client.handleMessage({ type: "rpc-reply", v: 1, id, ok: false, error: { message: "This catalog is unavailable.", loginRequired: false } });
  assert.equal(subscription.active, false);
  assert.equal(snapshots, 0);
  assert.equal(errors.length, 1);
  assert.equal(errors[0]?.message, "This catalog is unavailable.");
  assert.equal(errors[0]?.loginRequired, false);
  assert.deepEqual(client.streamNames(), []);
  client.close();
});
