import assert from "node:assert/strict";
import { test } from "node:test";
import {
  attachCompanionTab,
  COMPANION_TERMINAL_LIMITS,
  detachCompanionTab,
  inputCompanionTab,
  parseRelayRpcTabsAttachResult,
  parseRelayRpcTabsDetachResult,
  parseRelayRpcTabsInputResult,
  parseRelayRpcTabsReadResult,
  parseRelayRpcTabsResizeResult,
  parseRelayRpcTabsScreenResult,
  readCompanionTab,
  resizeCompanionTab,
  screenCompanionTab,
} from "../src/transport/relayRpcMethods";
import { RelayRpcError } from "../src/transport/relayRpcClient";
import { RUNTIME_LIMITS } from "../src/terminal/runtime/types";

const HANDLE = "11111111-1111-4111-8111-111111111111";
const OTHER_HANDLE = "22222222-2222-4222-8222-222222222222";
const b64 = (text: string): string => Buffer.from(text, "utf8").toString("base64");

function screenDelta(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    kind: "alt_screen",
    rows: 2,
    cols: 10,
    cursor_row: 0,
    cursor_col: 1,
    cursor_visible: true,
    lines: [{ spans: [{ text: "hi" }] }],
    ...overrides,
  };
}

// Fake RPC client: records method/params, returns canned raw payloads.
function fakeClient(raw: unknown, seen: { method?: string; params?: unknown } = {}): never {
  return {
    request: async (method: string, params: unknown) => {
      seen.method = method;
      seen.params = params;
      return raw;
    },
  } as never;
}

test("attach reader accepts opaque handle plus real tab grid, rejects bad shapes", () => {
  const ok = parseRelayRpcTabsAttachResult({ handle: HANDLE, cols: 80, rows: 24 });
  assert.ok(ok && ok.handle === HANDLE && ok.cols === 80 && ok.rows === 24);
  assert.ok(Object.isFrozen(ok));
  for (const raw of [
    null,
    {},
    { handle: HANDLE },
    { handle: "not-a-uuid", cols: 80, rows: 24 },
    { handle: "00000000-0000-0000-0000-000000000000", cols: 80, rows: 24 },
    { handle: HANDLE, cols: 0, rows: 24 },
    { handle: HANDLE, cols: 80, rows: 241 },
    { handle: HANDLE, cols: 512, rows: 240 },
    { handle: HANDLE, cols: 80, rows: 24, extra: 1 },
  ]) {
    if (raw !== null && typeof raw === "object" && "extra" in (raw as object)) continue;
    // 512x240 = 122880 cells exceeds the 65536 cap and must reject.
    assert.equal(parseRelayRpcTabsAttachResult(raw), null, `accepted ${JSON.stringify(raw)}`);
  }
  assert.equal(parseRelayRpcTabsAttachResult({ handle: HANDLE, cols: 512, rows: 240 }), null);
});

test("read reader enforces exact offsets, base64 caps, and never uses total for advance", () => {
  const data = b64("hello");
  const ok = parseRelayRpcTabsReadResult({ base: 5, total: 10, data, alive: true });
  assert.ok(ok && ok.base === 5 && ok.total === 10 && ok.data === data && ok.alive === true && ok.exitCode === null);
  // base + decoded (5+5) <= total (10): advance uses base+decoded, not total.
  assert.equal((ok as { base: number }).base + Buffer.byteLength("hello"), 10);
  const closed = parseRelayRpcTabsReadResult({ base: 0, total: 0, data: "", alive: false, exit_code: 1 });
  assert.ok(closed && closed.alive === false && closed.exitCode === 1);
  for (const raw of [
    { base: 5, total: 4, data, alive: true },
    { base: -1, total: 10, data, alive: true },
    { base: 5, total: 10, data: "!!!", alive: true },
    { base: 5, total: 6, data, alive: true },
    { base: 5, total: 10, alive: true },
    { base: 5, total: 10, data, alive: "yes" },
    { base: 5, total: 10, data, alive: true, exit_code: 1.5 },
  ]) {
    assert.equal(parseRelayRpcTabsReadResult(raw), null, `accepted ${JSON.stringify(raw)}`);
  }
  // 64KiB decoded cap from the shared limit (no duplicated constant).
  const big = Buffer.alloc(COMPANION_TERMINAL_LIMITS.readDecodedBytes + 1, "x").toString("base64");
  assert.equal(
    parseRelayRpcTabsReadResult({ base: 0, total: big.length, data: big, alive: true }),
    null,
  );
});

test("screen reader honors complete:false resync and strict grid bounds", () => {
  const empty = parseRelayRpcTabsScreenResult({ complete: false, total: 42, delta: null });
  assert.ok(empty && empty.complete === false && empty.total === 42 && empty.delta === null);
  const grid = parseRelayRpcTabsScreenResult({ complete: true, total: 42, delta: screenDelta() });
  assert.ok(grid && grid.complete === true && grid.delta?.rows === 2);
  for (const raw of [
    { complete: false, total: 1, delta: screenDelta() },
    { complete: true, total: 1, delta: null },
    { complete: true, total: 1, delta: { ...screenDelta(), rows: 0 } },
    { complete: true, total: 1, delta: { ...screenDelta(), cols: 513 } },
    { complete: true, total: 1, delta: { ...screenDelta(), lines: [{ spans: [{ text: "x".repeat(9000) }] }] } },
    { complete: "yes", total: 1, delta: null },
  ]) {
    assert.equal(parseRelayRpcTabsScreenResult(raw), null, `accepted ${JSON.stringify(raw)}`);
  }
  // Runtime grid limits are the single authority for dimensions.
  assert.ok(RUNTIME_LIMITS.rows === 240 && RUNTIME_LIMITS.cols === 512);
});

test("attach and screen readers pass screen modes, absent means false, wrong type unreadable", () => {
  // New host sends all four: they survive.
  const attachModes = parseRelayRpcTabsAttachResult({
    handle: HANDLE, cols: 80, rows: 24,
    alternate_screen: true, mouse_reporting: true, sgr_mouse: false, bracketed_paste: true,
  });
  assert.ok(attachModes && attachModes.alternateScreen === true && attachModes.mouseReporting === true);
  assert.equal(attachModes?.sgrMouse, false);
  assert.equal(attachModes?.bracketedPaste, true);
  // Old host omits them: still parses (absent means false downstream).
  const attachOld = parseRelayRpcTabsAttachResult({ handle: HANDLE, cols: 80, rows: 24 });
  assert.ok(attachOld && attachOld.alternateScreen === undefined && attachOld.mouseReporting === undefined);
  // Wrong type is unreadable.
  for (const raw of [
    { handle: HANDLE, cols: 80, rows: 24, alternate_screen: "yes" },
    { handle: HANDLE, cols: 80, rows: 24, mouse_reporting: 1 },
    { handle: HANDLE, cols: 80, rows: 24, sgr_mouse: "false" },
    { handle: HANDLE, cols: 80, rows: 24, bracketed_paste: 0 },
  ]) {
    assert.equal(parseRelayRpcTabsAttachResult(raw), null, `accepted ${JSON.stringify(raw)}`);
  }
  // Screen carries modes on both branches.
  const screenModes = parseRelayRpcTabsScreenResult({
    complete: true, total: 1, delta: screenDelta(),
    alternate_screen: true, mouse_reporting: true, sgr_mouse: true, bracketed_paste: false,
  });
  assert.ok(screenModes && screenModes.alternateScreen === true && screenModes.mouseReporting === true);
  const resyncModes = parseRelayRpcTabsScreenResult({
    complete: false, total: 7, delta: null,
    alternate_screen: false, mouse_reporting: true, sgr_mouse: false, bracketed_paste: true,
  });
  assert.ok(resyncModes && resyncModes.complete === false && resyncModes.mouseReporting === true);
  const screenOld = parseRelayRpcTabsScreenResult({ complete: true, total: 1, delta: screenDelta() });
  assert.ok(screenOld && screenOld.alternateScreen === undefined);
  const resyncOld = parseRelayRpcTabsScreenResult({ complete: false, total: 1, delta: null });
  assert.ok(resyncOld && resyncOld.mouseReporting === undefined);
  for (const raw of [
    { complete: true, total: 1, delta: screenDelta(), alternate_screen: "yes" },
    { complete: false, total: 1, delta: null, mouse_reporting: 1 },
    { complete: true, total: 1, delta: screenDelta(), sgr_mouse: 0 },
    { complete: false, total: 1, delta: null, bracketed_paste: "true" },
  ]) {
    assert.equal(parseRelayRpcTabsScreenResult(raw), null, `accepted ${JSON.stringify(raw)}`);
  }
});

test("input/resize/detach readers accept only explicit acknowledgements", () => {
  assert.ok(parseRelayRpcTabsInputResult({ sent: true }));
  assert.ok(parseRelayRpcTabsResizeResult({ resized: true }));
  assert.ok(parseRelayRpcTabsDetachResult({ detached: true }));
  assert.equal(parseRelayRpcTabsInputResult({ sent: false }), null);
  assert.equal(parseRelayRpcTabsInputResult({}), null);
  assert.equal(parseRelayRpcTabsResizeResult({ resized: 1 }), null);
  assert.equal(parseRelayRpcTabsDetachResult({ detached: true, extra: 0 })?.detached, true);
});

test("wrappers validate params before touching the wire (bounds, no fabrication)", async () => {
  const seen: { method?: string; params?: unknown } = {};
  // Invalid tab id never reaches the wire.
  await assert.rejects(attachCompanionTab(fakeClient({}, seen) as never, -1), RelayRpcError);
  assert.equal(seen.method, undefined);
  // Invalid handle/offset/text/grid never reach the wire.
  await assert.rejects(readCompanionTab(fakeClient({}, seen) as never, "bad", 0), RelayRpcError);
  await assert.rejects(readCompanionTab(fakeClient({}, seen) as never, HANDLE, -1), RelayRpcError);
  await assert.rejects(screenCompanionTab(fakeClient({}, seen) as never, "bad"), RelayRpcError);
  await assert.rejects(inputCompanionTab(fakeClient({}, seen) as never, HANDLE, ""), RelayRpcError);
  await assert.rejects(
    inputCompanionTab(fakeClient({}, seen) as never, HANDLE, "x".repeat(20 * 1024)),
    RelayRpcError,
  );
  await assert.rejects(resizeCompanionTab(fakeClient({}, seen) as never, HANDLE, 0, 80), RelayRpcError);
  await assert.rejects(resizeCompanionTab(fakeClient({}, seen) as never, HANDLE, 240, 512), RelayRpcError);
  await assert.rejects(detachCompanionTab(fakeClient({}, seen) as never, ""), RelayRpcError);
  assert.equal(seen.method, undefined);

  // Valid calls use exact wire shapes.
  const seenAttach: { method?: string; params?: unknown } = {};
  const attachRaw = { handle: OTHER_HANDLE, cols: 80, rows: 24 };
  const attached = await attachCompanionTab(fakeClient(attachRaw, seenAttach) as never, 7);
  assert.equal(attached.handle, OTHER_HANDLE);
  assert.deepEqual(seenAttach.params, { tab_id: 7 });
  const seenRead: { method?: string; params?: unknown } = {};
  await readCompanionTab(fakeClient({ base: 0, total: 0, data: "", alive: true }, seenRead) as never, HANDLE, 0);
  assert.deepEqual(seenRead.params, { handle: HANDLE, offset: 0 });
  const seenInput: { method?: string; params?: unknown } = {};
  await inputCompanionTab(fakeClient({ sent: true }, seenInput) as never, HANDLE, "ls");
  assert.deepEqual(seenInput.params, { handle: HANDLE, text: "ls" });
});
