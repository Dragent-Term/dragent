import assert from "node:assert/strict";
import { test } from "node:test";
import {
  CompanionTerminalEngine,
  partFailureOf,
  type CompanionTerminalRpc,
} from "../src/terminal/companion/engine";
import { createCompanionHandleRegistry } from "../src/terminal/companion/registry";
import {
  REPLY_FAILED_CODE,
  REPLY_FAILED_MESSAGE,
  SEED_FAILED_CODE,
  SEED_FAILED_MESSAGE,
} from "../src/transport/e2ee/desktopLink";
import { RelayRpcError } from "../src/transport/relayRpcClient";
import type { TerminalDelta } from "../src/transport/protocol";

const H1 = "11111111-1111-4111-8111-111111111111";
const b64 = (text: string): string => Buffer.from(text, "utf8").toString("base64");
const seedError = () => new RelayRpcError(SEED_FAILED_MESSAGE, false, SEED_FAILED_CODE);
const replyError = () => new RelayRpcError(REPLY_FAILED_MESSAGE, false, REPLY_FAILED_CODE);

function pollingRig(opts: {
  readImpl?: (handle: string, offset: number) => Promise<any>;
  screenImpl?: (handle: string) => Promise<any>;
  priorPartFailures?: number;
  noted?: number[];
} = {}) {
  const attached: number[] = [];
  const received: TerminalDelta[] = [];
  const rpc: CompanionTerminalRpc = {
    attach: async (tabId: number) => {
      attached.push(tabId);
      return { handle: H1, cols: 80, rows: 24 };
    },
    read: async (handle, offset) => {
      if (opts.readImpl) return opts.readImpl(handle, offset);
      return { base: offset, total: offset, data: "", alive: true, exitCode: null };
    },
    screen: async (handle) => {
      if (opts.screenImpl) return opts.screenImpl(handle);
      return { complete: true, total: 0, delta: null } as never;
    },
    input: async () => ({ sent: true }),
    resize: async () => ({ resized: true }),
    detach: async () => ({ detached: true }),
  };
  const engine = new CompanionTerminalEngine(
    { accountId: "a1", deviceId: "d1", tabId: 7, client: {}, epoch: 1 },
    rpc,
    {
      receive: (d) => received.push(d),
      reset: () => {},
      notePartFailure: () => {
        opts.noted?.push(1);
      },
    },
    createCompanionHandleRegistry(),
    undefined,
    undefined,
    opts.priorPartFailures ?? 0,
  );
  return { engine, attached, received };
}

test("partFailureOf only matches seed_failed and reply_failed", () => {
  assert.equal(partFailureOf(seedError())?.code, SEED_FAILED_CODE);
  assert.equal(partFailureOf(replyError())?.code, REPLY_FAILED_CODE);
  assert.equal(partFailureOf(new RelayRpcError("nope")), null);
  assert.equal(partFailureOf(new Error("nope")), null);
});

test("first seed failure is a manual-Retry error, never a reattach loop", async () => {
  const noted: number[] = [];
  const { engine, attached } = pollingRig({
    readImpl: async () => {
      throw seedError();
    },
    noted,
  });
  await engine.attach();
  assert.equal(engine.getStatus().phase, "live-raw");
  await engine.pollOnce();
  assert.equal(engine.getStatus().phase, "error");
  assert.equal(engine.getStatus().message, SEED_FAILED_MESSAGE);
  assert.equal(engine.getStatus().canWrite, false);
  assert.deepEqual(noted, [1]);
  const attaches = attached.length;
  for (let i = 0; i < 4; i++) await engine.pollOnce();
  assert.equal(attached.length, attaches, "no automatic reconnect loop");
  assert.equal(engine.getStatus().phase, "error");
  assert.equal(engine.getRawUsage().reattachAttempts, 0);
});

test("first reply failure on screen is a manual-Retry error too", async () => {
  const { engine } = pollingRig({
    readImpl: async () => ({ base: 9, total: 9, data: b64("x"), alive: true, exitCode: null }),
    screenImpl: async () => {
      throw replyError();
    },
  });
  await engine.attach();
  await engine.pollOnce();
  assert.equal(engine.getStatus().phase, "live-snapshot", "gap switches to snapshot mode");
  await engine.pollOnce();
  assert.equal(engine.getStatus().phase, "error");
  assert.equal(engine.getStatus().message, REPLY_FAILED_MESSAGE);
  assert.equal(engine.getRawUsage().reattachAttempts, 0);
});

test("second failure keeps the live stream without history", async () => {
  const { engine } = pollingRig({
    readImpl: async () => {
      throw seedError();
    },
    priorPartFailures: 1,
  });
  await engine.attach();
  await engine.pollOnce();
  const status = engine.getStatus();
  assert.equal(status.phase, "live-raw");
  assert.equal(status.message, SEED_FAILED_MESSAGE);
  assert.equal(status.canWrite, true);
  assert.equal(status.canResize, true);
  // Live output still flows after the history failure.
  await engine.pollOnce();
  assert.equal(engine.getStatus().phase, "live-raw");
  assert.equal(engine.getStatus().message, SEED_FAILED_MESSAGE);
});
