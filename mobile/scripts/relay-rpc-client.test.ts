import assert from "node:assert/strict";
import { test } from "node:test";
import { RelayRpcClient } from "../src/transport/relayRpcClient";
import type { RealtimeClock } from "../src/transport/clock";
import {
  createTerminal,
  createWorktree,
  listFiles,
  listGithubWorkItems,
  listGitlabWorkItems,
  listTabs,
  listWorktrees,
  openFile,
  parseRelayRpcAttentionEvent,
  parseRelayRpcEventsSnapshot,
  parseRelayRpcFileContentResult,
  parseRelayRpcTabsResult,
  parseRelayRpcWorkItemsResult,
  searchFiles,
  subscribeRuntimeEvents,
} from "../src/transport/relayRpcMethods";

function makeClock() {
  let now = 1000;
  let next = 1;
  const timers = new Map<number, { at: number; run: () => void }>();
  const clock: RealtimeClock = {
    now: () => now,
    setTimeout: (run: () => void, delayMs: number) => {
      const id = next++;
      timers.set(id, { at: now + delayMs, run });
      return id;
    },
    clearTimeout: (handle: unknown) => { timers.delete(handle as number); },
  };
  return {
    clock,
    advance(ms: number) {
      const end = now + ms;
      for (;;) {
        let best: [number, { at: number; run: () => void }] | null = null;
        for (const entry of timers) {
          if (entry[1].at <= end && (!best || entry[1].at < best[1].at)) best = entry;
        }
        if (!best) break;
        now = best[1].at;
        timers.delete(best[0]);
        best[1].run();
      }
      now = end;
    },
  };
}

function harness() {
  const sent: Record<string, unknown>[] = [];
  let ids = 0;
  const clock = makeClock();
  const client = new RelayRpcClient({
    send: (frame) => { sent.push(frame); return true; },
    createRequestId: () => `00000000-0000-4000-8000-${String(++ids).padStart(12, "0")}`,
    clock: clock.clock,
    log: () => {},
  });
  const reply = (id: string, ok: boolean, result: unknown, error?: { message: string; loginRequired: boolean }) =>
    client.handleMessage(JSON.stringify(
      ok
        ? { type: "rpc-reply", v: 1, id, ok: true, result }
        : { type: "rpc-reply", v: 1, id, ok: false, error },
    ));
  const lastSent = () => sent[sent.length - 1] as Record<string, unknown>;
  return { client, sent, clock, reply, lastSent };
}

test("listTabs sends a versioned request and parses the result", async () => {
  const { client, sent, reply, lastSent } = harness();
  const pending = listTabs(client);
  assert.equal(sent.length, 1);
  assert.equal(lastSent().type, "rpc-request");
  assert.equal(lastSent().method, "tabs.list");
  assert.equal(lastSent().v, 1);
  reply(lastSent().id as string, true, { tabs: [{ id: 3, kind: "terminal", title: "Term", active: true, visible: true }] });
  assert.deepEqual(await pending, [{ id: 3, kind: "terminal", title: "Term", active: true, visible: true }]);
});

test("a refusal keeps the desktop's reason code beside its words", async () => {
  // The phone leaves a tab when the desktop says `desktop_in_control`: the
  // code must reach the screen even though the reply also carries a message.
  const { client, reply, lastSent } = harness();
  const pending = listTabs(client);
  reply(lastSent().id as string, false, null, {
    message: "Your desktop is using this terminal.", loginRequired: false, code: "desktop_in_control",
  } as { message: string; loginRequired: boolean });
  await assert.rejects(pending, (error: unknown) => (error as { code?: unknown }).code === "desktop_in_control");
  // A code that isn't a plain short word is dropped, never passed on.
  const odd = listTabs(client);
  reply(lastSent().id as string, false, null, {
    message: "No.", loginRequired: false, code: "<b>x</b>",
  } as { message: string; loginRequired: boolean });
  await assert.rejects(odd, (error: unknown) => (error as { code?: unknown }).code === null);
});

test("ok:false propagates loginRequired for auth failures", async () => {
  const { client, reply, lastSent, sent } = harness();
  const pending = listTabs(client);
  assert.equal(sent.length, 1);
  reply(lastSent().id as string, false, null, { message: "login required", loginRequired: true });
  await assert.rejects(pending, (error: unknown) =>
    error instanceof Error && error.message === "login required" &&
    (error as { loginRequired?: boolean }).loginRequired === true);
});

test("malformed results reject unreadable without leaking the payload", async () => {
  const { client, reply, lastSent } = harness();
  const pending = listTabs(client);
  reply(lastSent().id as string, true, { tabs: [{ id: "nope", kind: "terminal", title: "x", active: true, visible: true }] });
  await assert.rejects(pending, (error: unknown) =>
    error instanceof Error && !JSON.stringify(error.message).includes("nope") &&
    error.message === "The relay response was unreadable.");
});

test("timeouts reject locally and never retry", async () => {
  const { client, sent, clock } = harness();
  const pending = listTabs(client);
  clock.advance(15_000);
  await assert.rejects(pending, /timed out/);
  assert.equal(sent.length, 1);
});

test("oversized and invalid inputs are rejected without sending", async () => {
  const { client, sent } = harness();
  await assert.rejects(listFiles(client, "   "), /invalid/);
  await assert.rejects(searchFiles(client, ""), /invalid/);
  await assert.rejects(openFile(client, ""), /invalid/);
  await assert.rejects(createWorktree(client, { name: "" }), /invalid/);
  await assert.rejects(listGithubWorkItems(client, { query: "x", repo: "a/b", project: "c" } as never), /invalid/);
  const big = `https://example.test/${"x".repeat(70 * 1024)}`;
  await assert.rejects(listGithubWorkItems(client, { query: big }), /invalid|too large/);
  assert.equal(sent.length, 0);
});

test("worktree, files and provider methods pass params through", async () => {
  const { client, sent, reply, lastSent } = harness();
  const p1 = createWorktree(client, { name: "ws", branch: "feat/a" });
  reply(lastSent().id as string, true, { name: "ws", path: "/w", branch: "feat/a", project: null });
  assert.deepEqual(await p1, { name: "ws", path: "/w", branch: "feat/a", project: null });
  const sentCreate = lastSent();
  assert.deepEqual(sentCreate.params, { name: "ws", branch: "feat/a" });

  const p2 = listGithubWorkItems(client, { query: "crash", repo: "acme/app" });
  assert.deepEqual(lastSent().params, { query: "crash", repo: "acme/app" });
  reply(lastSent().id as string, true, {
    items: [{ id: "1", number: 42, title: "Crash", url: "https://example.test/i/42", state: "open", repo: "acme/app" }],
  });
  assert.equal((await p2)[0]?.number, 42);

  const p3 = listFiles(client, "src");
  assert.deepEqual(lastSent().params, { path: "src" });
  reply(lastSent().id as string, true, { entries: [{ name: "a.ts", kind: "file" }] });
  assert.equal((await p3)[0]?.name, "a.ts");

  const p4 = listGitlabWorkItems(client, { query: "leak", project: "acme/api" });
  assert.deepEqual(lastSent().params, { query: "leak", project: "acme/api" });
  reply(lastSent().id as string, true, { items: [] });
  assert.deepEqual(await p4, []);

  const p5 = listWorktrees(client);
  assert.deepEqual(lastSent().params, {});
  reply(lastSent().id as string, true, {
    worktrees: [{ name: "Nav", path: "/repo", branch: "main", project: "Web" }],
  });
  assert.equal((await p5)[0]?.branch, "main");

  const p6 = createTerminal(client);
  assert.equal(lastSent().method, "tabs.createTerminal");
  reply(lastSent().id as string, true, { id: 9 });
  assert.equal((await p6).id, 9);
  assert.equal(sent.filter((frame) => frame.type === "rpc-request").length, 6);
});

test("runtime events start from a snapshot, then deliver ordered events", async () => {
  const { client, sent, reply } = harness();
  const snapshots: unknown[][] = [];
  const events: unknown[] = [];
  const sub = subscribeRuntimeEvents(client, {
    onSnapshot: (list) => { snapshots.push([...list]); },
    onEvent: (event) => { events.push(event); },
  });
  void reply;
  const subscribe = sent.find((frame) => frame.type === "rpc-subscribe");
  assert.ok(subscribe);
  const stream = (subscribe as Record<string, unknown>).stream;
  assert.equal(stream, "runtime.events");
  // Snapshot first.
  client.handleMessage(JSON.stringify({
    type: "rpc-stream", v: 1, stream, seq: 1,
    payload: { events: [{ kind: "agent_completed", title: "Done", body: "ok", sessionId: null, occurredAt: null, data: {} }] },
  }));
  assert.equal(snapshots.length, 1);
  assert.equal(events.length, 0);
  // Ordered live event.
  client.handleMessage(JSON.stringify({
    type: "rpc-stream", v: 1, stream, seq: 2,
    payload: { kind: "control_granted", title: "Edit on", body: "ok", sessionId: null, occurredAt: null, data: {} },
  }));
  assert.equal(events.length, 1);
  // Malformed payload is dropped without breaking the sequence.
  client.handleMessage(JSON.stringify({ type: "rpc-stream", v: 1, stream, seq: 3, payload: { kind: "nope" } }));
  assert.equal(events.length, 1);
  client.handleMessage(JSON.stringify({
    type: "rpc-stream", v: 1, stream, seq: 4,
    payload: { kind: "join_approved", title: "Hi", body: "ok", sessionId: null, occurredAt: null, data: {} },
  }));
  assert.equal(events.length, 2);
  // Gap triggers a resubscribe, never a partial sequence.
  const before = sent.filter((frame) => frame.type === "rpc-subscribe").length;
  client.handleMessage(JSON.stringify({
    type: "rpc-stream", v: 1, stream, seq: 9,
    payload: { kind: "join_approved", title: "Hi", body: "ok", sessionId: null, occurredAt: null, data: {} },
  }));
  assert.ok(sent.filter((frame) => frame.type === "rpc-subscribe").length > before);
  // The held unsubscribe confirms once the fresh snapshot lands.
  const unsubscribed = sub.unsubscribe();
  client.handleMessage(JSON.stringify({ type: "rpc-stream", v: 1, stream, seq: 1, payload: { events: [] } }));
  await unsubscribed;
  assert.equal(snapshots.length, 2);
});

test("reader units reject bad shapes and hostile values", () => {
  assert.equal(parseRelayRpcTabsResult({ tabs: [{ id: -1, kind: "terminal", title: "x", active: true, visible: true }] }), null);
  assert.equal(parseRelayRpcTabsResult({ tabs: [{ id: 1, kind: "agent", title: "x", active: true, visible: true }] }), null);
  assert.equal(parseRelayRpcWorkItemsResult({
    items: [{ id: "1", number: 1, title: "t", url: "javascript:alert(1)", state: "open", repo: "a/b" }],
  }), null);
  assert.equal(parseRelayRpcFileContentResult({ content: "x".repeat(5 * 1024 * 1024), truncated: true }), null);
  assert.equal(parseRelayRpcAttentionEvent({ kind: "bogus", title: "t", body: "b" }), null);
  const event = parseRelayRpcAttentionEvent({ kind: "agent_completed", title: "t", body: "b", session_id: "s", extra: 1 });
  assert.deepEqual(event, { kind: "agent_completed", title: "t", body: "b", sessionId: "s", occurredAt: null, data: {} });
  assert.equal(parseRelayRpcEventsSnapshot({ events: [] })?.length, 0);
});
