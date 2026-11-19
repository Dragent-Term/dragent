import assert from "node:assert/strict";
import { test } from "node:test";
import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";
import {
  COORDINATION_LIMITS,
  parseCoordinationDetailResult,
  parseCoordinationListResult,
} from "../src/coordination/readers";
import { getCoordination, listCoordinations } from "../src/coordination/rpc";
import {
  CoordinationResourceController,
  coordinationReasonOf,
  type CoordinationResourceState,
} from "../src/coordination/controller";
import {
  coordinationClientTokenFor,
  coordinationOwnerKey,
  COORDINATION_POLL_MS,
  isCoordinationEligible,
  selectOwnedCoordination,
} from "../src/coordination/owner";
import { RelayRpcClient, RelayRpcError } from "../src/transport/relayRpcClient";
import {
  machineCoordinationDetailPath,
  singleParam,
} from "../src/machines/routes";

const require = createRequire(import.meta.url);
const { resolveHref } = require("expo-router/build/link/href") as {
  resolveHref: (href: { pathname: string; params?: Record<string, string> }) => string;
};

function hookDecode(value: string): string {
  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
}

function listItem(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    id: "team-abc123",
    title: "Ship feature",
    project_path: "/repo/web",
    status: "running",
    task_count: 3,
    done_count: 1,
    running_count: 1,
    blocked_count: 0,
    needs_user: false,
    updated_at: 1_700_000_000_000,
    ...overrides,
  };
}

function teamPayload(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    team: {
      id: "team-abc123",
      title: "Ship feature",
      project_path: "/repo/web",
      status: "running",
      goal: "Land the feature behind a flag.",
      tasks: [
        {
          id: "t1",
          title: "Implement",
          status: "running",
          agent: "codex",
          model: "gpt-5",
          depends_on: [],
          result_summary: null,
        },
        {
          id: "t2",
          title: "Review",
          status: "pending",
          agent: null,
          model: null,
          depends_on: ["t1"],
          result_summary: "waiting on t1",
        },
      ],
      messages: [
        {
          id: "m1",
          from: "lead",
          label: "lead",
          kind: "note",
          text: "Starting t1.",
          created_at: 1_700_000_000_000,
        },
      ],
      ...((overrides.team ?? {}) as Record<string, unknown>),
    },
    truncated: false,
    ...overrides,
  };
}

function fakeClient(raw: unknown, seen: { method?: string; params?: unknown } = {}): never {
  return {
    request: async (method: string, params: unknown) => {
      seen.method = method;
      seen.params = params;
      return raw;
    },
  } as never;
}

function deferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (error: unknown) => void;
} {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

test("coordination poll cadence stays bounded at 5s or slower", () => {
  assert.ok(COORDINATION_POLL_MS >= 5000);
});

test("owner key changes on any identity/scope field; never collides across boundaries", () => {
  const base = {
    accountId: "a1",
    deviceId: "d1",
    epoch: 1,
    retry: 0,
    clientToken: "client:1",
    projectId: "",
    teamId: "team-1",
  };
  const key = coordinationOwnerKey(base);
  assert.equal(coordinationOwnerKey({ ...base }), key);
  for (const next of [
    { ...base, accountId: "a2" },
    { ...base, accountId: null },
    { ...base, deviceId: "d2" },
    { ...base, epoch: 2 },
    { ...base, retry: 1 },
    { ...base, clientToken: "client:2" },
    { ...base, projectId: "/repo" },
    { ...base, teamId: "team-2" },
  ]) {
    assert.notEqual(coordinationOwnerKey(next), key);
  }
  // Boundary collisions under plain concatenation stay distinct as a tuple.
  const keyed = (fields: typeof base) => coordinationOwnerKey(fields);
  assert.notEqual(
    keyed({ ...base, accountId: "ab", deviceId: "c" }),
    keyed({ ...base, accountId: "a", deviceId: "bc" }),
  );
  assert.notEqual(
    keyed({ ...base, deviceId: 'd","x', projectId: "p" }),
    keyed({ ...base, deviceId: "d", projectId: 'x","p' }),
  );
  assert.notEqual(
    keyed({ ...base, projectId: "ab", teamId: "c" }),
    keyed({ ...base, projectId: "a", teamId: "bc" }),
  );
  // Numbers stay numbers: epoch 1 never equals the string "1" in another slot.
  assert.notEqual(
    keyed({ ...base, deviceId: "1", epoch: 2 }),
    keyed({ ...base, deviceId: "2", epoch: 1 }),
  );
  assert.equal(coordinationClientTokenFor(null), "none");
  const a = {};
  assert.equal(coordinationClientTokenFor(a), coordinationClientTokenFor(a));
  assert.notEqual(coordinationClientTokenFor(a), coordinationClientTokenFor({}));
});

test("eligibility requires every gate including an active session; no partial credit", () => {
  const eligible = {
    foreground: true,
    socketReady: true,
    sessionActive: true,
    online: true,
    rpcAvailable: true,
    deviceValid: true,
  };
  assert.equal(isCoordinationEligible(eligible), true);
  for (const field of Object.keys(eligible) as Array<keyof typeof eligible>) {
    assert.equal(isCoordinationEligible({ ...eligible, [field]: false }), false, field);
  }
  assert.equal(selectOwnedCoordination("k1", "k1", "live", "blank"), "live");
  assert.equal(selectOwnedCoordination("k-old", "k-new", "live", "blank"), "blank");
});

test("list reader accepts the native contract shape and freezes it", () => {
  const parsed = parseCoordinationListResult({ items: [listItem()], truncated: true });
  assert.ok(parsed);
  assert.equal(parsed!.items.length, 1);
  assert.equal(parsed!.truncated, true);
  assert.ok(Object.isFrozen(parsed));
  assert.ok(Object.isFrozen(parsed!.items));
  const item = parsed!.items[0]!;
  assert.equal(item.id, "team-abc123");
  assert.equal(item.status, "running");
  assert.equal(parseCoordinationListResult({ items: [], truncated: false })?.items.length, 0);
});

test("list reader stays strict: bad enums, counts, flags and bounds reject", () => {
  for (const raw of [
    null,
    {},
    { items: [], truncated: "no" },
    { items: null, truncated: false },
    { items: [listItem({ status: "working" })], truncated: false },
    { items: [listItem({ status: "napping" })], truncated: false },
    { items: [listItem({ task_count: -1 })], truncated: false },
    { items: [listItem({ done_count: 1.5 })], truncated: false },
    { items: [listItem({ done_count: 4 })], truncated: false },
    { items: [listItem({ running_count: 4 })], truncated: false },
    { items: [listItem({ blocked_count: 4 })], truncated: false },
    { items: [listItem({ needs_user: "no" })], truncated: false },
    { items: [listItem({ id: "" })], truncated: false },
    { items: [listItem({ id: "x".repeat(129) })], truncated: false },
    { items: [listItem({ title: "x".repeat(2049) })], truncated: false },
    { items: [listItem({ updated_at: -1 })], truncated: false },
    { items: [listItem({ updated_at: 2 ** 53 })], truncated: false },
    { items: [listItem({ updated_at: 1.5 })], truncated: false },
    { items: [listItem({ updated_at: "1700000000000" })], truncated: false },
    { items: [listItem(), listItem()], truncated: false },
    { items: Array.from({ length: 101 }, (_, i) => listItem({ id: `team-${i}` })), truncated: false },
  ]) {
    assert.equal(parseCoordinationListResult(raw), null, `accepted ${JSON.stringify(raw)?.slice(0, 120)}`);
  }
  // IDs are verbatim: surrounding whitespace is preserved, never trimmed.
  const spaced = parseCoordinationListResult({ items: [listItem({ id: " team-1" })], truncated: false });
  assert.ok(spaced && spaced.items[0]!.id === " team-1");
});

test("list reader ignores nothing: camelCase aliases are not read", () => {
  assert.equal(
    parseCoordinationListResult({
      items: [{
        id: "team-1",
        title: "T",
        status: "planning",
        projectPath: "/repo/web",
        taskCount: 3,
        doneCount: 1,
        runningCount: 1,
        blockedCount: 0,
        needsUser: false,
        updatedAt: 1,
      }],
      truncated: false,
    }),
    null,
  );
});

test("detail reader honors daemon team/task/message enums and truncation", () => {
  const ok = parseCoordinationDetailResult(teamPayload());
  assert.ok(ok && ok.team.tasks.length === 2 && ok.team.messages.length === 1);
  assert.equal(ok!.truncated, false);
  assert.ok(Object.isFrozen(ok!.team.tasks));
  for (const status of ["pending", "starting", "running", "blocked", "done", "failed", "needs_decision", "cancelled"]) {
    const payload = teamPayload({
      team: {
        id: "t",
        title: "T",
        project_path: "/p",
        status: "planning",
        goal: "g",
        tasks: [{ id: "t1", title: "A", status, agent: null, model: null, depends_on: [], result_summary: null }],
        messages: [],
      },
    });
    assert.ok(parseCoordinationDetailResult(payload), `rejected task status ${status}`);
  }
  for (const kind of ["note", "question", "answer", "handoff", "finding", "needs_user"]) {
    const payload = teamPayload({
      team: {
        id: "t",
        title: "T",
        project_path: "/p",
        status: "planning",
        goal: "g",
        tasks: [],
        messages: [{ id: "m1", from: "lead", label: "lead", kind, text: "hi", created_at: 1 }],
      },
    });
    assert.ok(parseCoordinationDetailResult(payload), `rejected kind ${kind}`);
  }
  for (const from of ["lead", "user", "task", "dragent"]) {
    const payload = teamPayload({
      team: {
        id: "t",
        title: "T",
        project_path: "/p",
        status: "planning",
        goal: "g",
        tasks: [],
        messages: [{ id: "m1", from, label: "t1", kind: "note", text: "hi", created_at: 1 }],
      },
    });
    assert.ok(parseCoordinationDetailResult(payload), `rejected from ${from}`);
  }
});

test("detail reader rejects duplicates, inconsistent detail and out-of-limit replies", () => {
  const dupTasks = teamPayload({
    team: {
      id: "t",
      title: "T",
      project_path: "/p",
      status: "planning",
      goal: "g",
      tasks: [
        { id: "t1", title: "A", status: "pending", agent: null, model: null, depends_on: [], result_summary: null },
        { id: "t1", title: "B", status: "pending", agent: null, model: null, depends_on: [], result_summary: null },
      ],
      messages: [],
    },
  });
  assert.equal(parseCoordinationDetailResult(dupTasks), null);
  const dupMessages = teamPayload({
    team: {
      id: "t",
      title: "T",
      project_path: "/p",
      status: "planning",
      goal: "g",
      tasks: [],
      messages: [
        { id: "m1", from: "lead", label: "lead", kind: "note", text: "a", created_at: 1 },
        { id: "m1", from: "lead", label: "lead", kind: "note", text: "b", created_at: 2 },
      ],
    },
  });
  assert.equal(parseCoordinationDetailResult(dupMessages), null);
  for (const raw of [
    teamPayload({ truncated: "no" }),
    teamPayload({ team: null, truncated: false }),
    teamPayload({ team: { id: "t" } }),
    teamPayload({
      team: {
        id: "t",
        title: "T",
        project_path: "/p",
        status: "planning",
        goal: "g",
        tasks: null,
        messages: [],
      },
    }),
    teamPayload({
      team: {
        id: "t",
        title: "T",
        project_path: "/p",
        status: "planning",
        goal: "g",
        tasks: [],
        messages: null,
      },
    }),
    teamPayload({
      team: {
        id: "t",
        title: "T",
        project_path: "/p",
        status: "planning",
        goal: "g",
        tasks: [{ id: "t1", title: "A", status: "working", agent: null, model: null, depends_on: [], result_summary: null }],
        messages: [],
      },
    }),
    teamPayload({
      team: {
        id: "t",
        title: "T",
        project_path: "/p",
        status: "planning",
        goal: "g",
        tasks: [],
        messages: [{ id: "m1", from: "lead", label: "lead", kind: "shout", text: "hi", created_at: 1 }],
      },
    }),
    teamPayload({
      team: {
        id: "t",
        title: "T",
        project_path: "/p",
        status: "planning",
        goal: "g",
        tasks: [],
        messages: [{ id: "m1", from: "you", label: "you", kind: "note", text: "hi", created_at: 1 }],
      },
    }),
    teamPayload({
      team: {
        id: "t",
        title: "T",
        project_path: "/p",
        status: "planning",
        goal: "g",
        tasks: [],
        messages: [{ id: 7, from: "lead", label: "lead", kind: "note", text: "hi", created_at: 1 }],
      },
    }),
    teamPayload({
      team: {
        id: "t",
        title: "T",
        project_path: "/p",
        status: "planning",
        goal: "x".repeat(32769),
        tasks: [],
        messages: [],
      },
    }),
    teamPayload({
      team: {
        id: "t",
        title: "T",
        project_path: "/p",
        status: "planning",
        goal: "g",
        tasks: [],
        messages: [{ id: "m1", from: "lead", label: "lead", kind: "note", text: "x".repeat(32769), created_at: 1 }],
      },
    }),
    teamPayload({
      team: {
        id: "t",
        title: "T",
        project_path: "/p",
        status: "planning",
        goal: "g",
        tasks: [{ id: "", title: "A", status: "pending", agent: null, model: null, depends_on: [], result_summary: null }],
        messages: [],
      },
    }),
    teamPayload({
      team: {
        id: "t",
        title: "T",
        project_path: "/p",
        status: "planning",
        goal: "g",
        tasks: [{ id: "t1", title: "A", status: "pending", agent: "x".repeat(513), model: null, depends_on: [], result_summary: null }],
        messages: [],
      },
    }),
    teamPayload({
      team: {
        id: "t",
        title: "T",
        project_path: "",
        status: "planning",
        goal: "g",
        tasks: [],
        messages: [],
      },
    }),
    teamPayload({
      team: {
        id: "t",
        title: "T",
        project_path: "/p",
        status: "planning",
        goal: "g",
        tasks: [],
        messages: [{ id: "m1", from: "lead", label: "lead", kind: "note", text: "hi", created_at: 2 ** 53 }],
      },
    }),
  ]) {
    assert.equal(parseCoordinationDetailResult(raw), null);
  }
  // Native bounds are respected (broker maxima, not invented extras).
  assert.equal(COORDINATION_LIMITS.items, 100);
  assert.equal(COORDINATION_LIMITS.tasks, 60);
  assert.equal(COORDINATION_LIMITS.messages, 100);
  const tooManyTasks = Array.from({ length: 61 }, (_, i) => (
    { id: `t${i}`, title: "A", status: "pending", agent: null, model: null, depends_on: [], result_summary: null }
  ));
  assert.equal(
    parseCoordinationDetailResult(teamPayload({ team: { id: "t", title: "T", project_path: "/p", status: "planning", goal: "g", tasks: tooManyTasks, messages: [] } })),
    null,
  );
  const tooManyMessages = Array.from({ length: 101 }, (_, i) => (
    { id: `m${i}`, from: "lead", label: "lead", kind: "note", text: "hi", created_at: i }
  ));
  assert.equal(
    parseCoordinationDetailResult(teamPayload({ team: { id: "t", title: "T", project_path: "/p", status: "planning", goal: "g", tasks: [], messages: tooManyMessages } })),
    null,
  );
  const tooManyDepends = Array.from({ length: 61 }, (_, i) => `t${i}`);
  assert.equal(
    parseCoordinationDetailResult(teamPayload({ team: { id: "t", title: "T", project_path: "/p", status: "planning", goal: "g", tasks: [{ id: "t1", title: "A", status: "pending", agent: null, model: null, depends_on: tooManyDepends, result_summary: null }], messages: [] } })),
    null,
  );
});

test("detail reader ignores nothing: camelCase/body/at aliases are not read", () => {
  assert.equal(
    parseCoordinationDetailResult({
      team: {
        id: "t",
        title: "T",
        projectPath: "/p",
        status: "planning",
        goal: "g",
        tasks: [],
        messages: [],
      },
      truncated: false,
    }),
    null,
  );
  assert.equal(
    parseCoordinationDetailResult(teamPayload({
      team: {
        id: "t",
        title: "T",
        project_path: "/p",
        status: "planning",
        goal: "g",
        tasks: [{ id: "t1", title: "A", status: "pending", agent: null, model: null, dependsOn: [], resultSummary: null }],
        messages: [],
      },
    })),
    null,
  );
  assert.equal(
    parseCoordinationDetailResult(teamPayload({
      team: {
        id: "t",
        title: "T",
        project_path: "/p",
        status: "planning",
        goal: "g",
        tasks: [],
        messages: [{ id: "m1", from: "lead", label: "lead", kind: "note", body: "hi", created_at: 1 }],
      },
    })),
    null,
  );
  assert.equal(
    parseCoordinationDetailResult(teamPayload({
      team: {
        id: "t",
        title: "T",
        project_path: "/p",
        status: "planning",
        goal: "g",
        tasks: [],
        messages: [{ id: "m1", from: "lead", label: "lead", kind: "note", text: "hi", createdAt: 1 }],
      },
    })),
    null,
  );
});

test("wrappers validate params before touching the wire and use exact shapes", async () => {
  const seen: { method?: string; params?: unknown } = {};
  await assert.rejects(listCoordinations(fakeClient({}, seen) as never, { project_id: "" }), RelayRpcError);
  await assert.rejects(getCoordination(fakeClient({}, seen) as never, ""), RelayRpcError);
  await assert.rejects(getCoordination(fakeClient({}, seen) as never, "team 1!"), RelayRpcError);
  assert.equal(seen.method, undefined);

  const seenList: { method?: string; params?: unknown } = {};
  const listed = await listCoordinations(
    fakeClient({ items: [listItem()], truncated: false }, seenList) as never,
    {},
  );
  assert.equal(seenList.method, "coordination.list");
  assert.deepEqual(seenList.params, {});
  assert.equal(listed.items.length, 1);

  const seenScoped: { method?: string; params?: unknown } = {};
  await listCoordinations(
    fakeClient({ items: [], truncated: true }, seenScoped) as never,
    { project_id: "/repo/web" },
  );
  assert.deepEqual(seenScoped.params, { project_id: "/repo/web" });

  const seenGet: { method?: string; params?: unknown } = {};
  const detail = await getCoordination(
    fakeClient(teamPayload(), seenGet) as never,
    "team-abc123",
  );
  assert.equal(seenGet.method, "coordination.get");
  assert.deepEqual(seenGet.params, { team_id: "team-abc123" });
  assert.equal(detail.team.id, "team-abc123");
});

test("malformed and mismatched replies fail closed with the single unreadable message", async () => {
  await assert.rejects(
    listCoordinations(fakeClient({ items: [{ id: "x" }], truncated: false }) as never, {}),
    /unreadable/,
  );
  await assert.rejects(
    getCoordination(fakeClient({ team: { id: "x" } }) as never, "team-1"),
    /unreadable/,
  );
  // Right shape, wrong team: the broker echoes the request, so mismatch fails closed.
  await assert.rejects(
    getCoordination(fakeClient(teamPayload({ team: { id: "team-other" } })) as never, "team-abc123"),
    /unreadable/,
  );
});

test("controller publishes one live result per fetch and never overlaps", async () => {
  let calls = 0;
  const gate = deferred<string>();
  const published: unknown[] = [];
  const controller = new CoordinationResourceController<string>(
    () => {
      calls += 1;
      return gate.promise;
    },
    (state) => {
      published.push(state);
    },
  );
  const first = controller.pollOnce();
  assert.equal(controller.fetching, true);
  // A second poll while the first is unresolved never touches the wire.
  await controller.pollOnce();
  assert.equal(calls, 1);
  gate.resolve("v1");
  await first;
  assert.equal(controller.fetching, false);
  assert.deepEqual(published, [{ status: "live", result: "v1" }]);
  // Settled controllers poll again on demand.
  await controller.pollOnce();
  assert.equal(calls, 2);
  controller.dispose();
});

test("controller drops deferred responses after cancel and after dispose", async () => {
  const gate = deferred<string>();
  const published: CoordinationResourceState<string>[] = [];
  const controller = new CoordinationResourceController<string>(
    () => gate.promise,
    (state) => {
      published.push(state);
    },
  );
  const pending = controller.pollOnce();
  controller.cancel();
  gate.resolve("late");
  await pending;
  assert.equal(published.length, 0);

  // A fresh generation on the same controller publishes again.
  const gate2 = deferred<string>();
  const controller2 = new CoordinationResourceController<string>(
    () => gate2.promise,
    (state) => {
      published.push(state);
    },
  );
  const pending2 = controller2.pollOnce();
  controller2.dispose();
  gate2.resolve("late");
  await pending2;
  assert.equal(published.length, 0);
  // Disposed controllers never fetch again.
  let calls = 0;
  let unexpected = 0;
  const dead = new CoordinationResourceController<string>(
    () => {
      calls += 1;
      return Promise.resolve("x");
    },
    () => {
      unexpected += 1;
    },
  );
  dead.dispose();
  await dead.pollOnce();
  assert.equal(calls, 0);
  assert.equal(unexpected, 0);
  assert.equal(published.length, 0);
});

test("controller maps failures to retryable unavailable with closed reasons", async () => {
  const published: unknown[] = [];
  const failing = new CoordinationResourceController<string>(
    () => Promise.reject(new RelayRpcError("The request failed.")),
    (state) => {
      published.push(state);
    },
  );
  await failing.pollOnce();
  assert.deepEqual(published, [{ status: "unavailable", reason: "The request failed.", action: "retry" }]);

  const unreadable: unknown[] = [];
  const malformed = new CoordinationResourceController<string>(
    () => Promise.reject(new RelayRpcError("The relay response was unreadable.")),
    (state) => {
      unreadable.push(state);
    },
  );
  await malformed.pollOnce();
  assert.deepEqual(unreadable, [{ status: "unavailable", reason: "The relay response was unreadable.", action: "retry" }]);

  const signedOut: unknown[] = [];
  const login = new CoordinationResourceController<string>(
    () => Promise.reject(new RelayRpcError("Sign in", true)),
    (state) => {
      signedOut.push(state);
    },
  );
  await login.pollOnce();
  assert.deepEqual(signedOut, [{ status: "unavailable", reason: "Pair this phone to view live coordination.", action: "retry" }]);

  const transport: unknown[] = [];
  const lost = new CoordinationResourceController<string>(
    () => Promise.reject(new Error("socket hung up")),
    (state) => {
      transport.push(state);
    },
  );
  await lost.pollOnce();
  assert.deepEqual(transport, [{ status: "unavailable", reason: "Live coordination is unreachable right now.", action: "retry" }]);

  assert.equal(coordinationReasonOf(new Error("nope")), "Live coordination is unreachable right now.");
});

test("late replies after close never land (generation/cancel lifecycle)", async () => {
  let ids = 0;
  const sent: Record<string, unknown>[] = [];
  const client = new RelayRpcClient({
    send: (frame) => {
      sent.push(frame);
      return true;
    },
    createRequestId: () => `00000000-0000-4000-8000-${String(++ids).padStart(12, "0")}`,
  });
  const pending = listCoordinations(client as never, {});
  const request = sent.at(-1)!;
  const rejected = assert.rejects(pending, /Connection lost/);
  client.close();
  await rejected;
  client.handleMessage({ type: "rpc-reply", v: 1, id: request.id, ok: true, result: { items: [], truncated: false } });
  assert.equal(client.pendingCount(), 0);
});

test("coordination detail route survives router encode/decode and resolves to a file", () => {
  const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
  assert.ok(
    existsSync(path.join(root, "app/machine/[deviceId]/coordination/[coordinationId].tsx")),
    "missing live coordination detail route",
  );
  for (const teamId of ["team-abc123", "team-1_2-3", "team-xyz"]) {
    const href = resolveHref(machineCoordinationDetailPath("desktop-1", teamId));
    const segments = href.split("/");
    assert.ok(segments.length >= 5, href);
    const rawTeam = segments.at(-1)!;
    assert.equal(singleParam(hookDecode(hookDecode(rawTeam))), teamId, teamId);
  }
  assert.equal(singleParam(undefined), "");
  assert.equal(singleParam(["a", "b"]), "");
});
