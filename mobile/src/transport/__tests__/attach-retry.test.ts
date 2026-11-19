// After a reconnect (a Wi-Fi switch) a tab's attach can run before the
// channel is back: it retries instead of sticking on "unavailable".
import assert from "node:assert/strict";
import { test } from "node:test";

import { attachRetryOf } from "../../terminal/companion/engine";
import { COMPANION_TERMINAL_MESSAGES } from "../../terminal/companion/types";
import { RELAY_RPC_CONNECTION_LOST_MESSAGE, RELAY_RPC_TIMEOUT_MESSAGE } from "../relayRpcClient";

test("transient failures retry for a while, real ones don't", () => {
  const reconnecting = COMPANION_TERMINAL_MESSAGES.reconnecting;
  assert.equal(attachRetryOf({ code: "channel_lost" }, 1), reconnecting);
  assert.equal(attachRetryOf({ message: RELAY_RPC_CONNECTION_LOST_MESSAGE }, 3), reconnecting);
  assert.equal(attachRetryOf({ message: RELAY_RPC_TIMEOUT_MESSAGE }, 14), reconnecting);
  assert.equal(attachRetryOf({ code: "channel_lost" }, 15), null, "gives up after ~30 s");
  assert.equal(attachRetryOf({ code: "unavailable" }, 1), reconnecting, "the desktop may still be re-registering");
  assert.equal(attachRetryOf({ code: "unavailable" }, 4), null, "then believe it");
  assert.equal(attachRetryOf({ code: "waking" }, 1), COMPANION_TERMINAL_MESSAGES.waking);
  assert.equal(attachRetryOf({ code: "invalid" }, 1), null);
  assert.equal(attachRetryOf({ code: "desktop_in_control" }, 1), null);
});
