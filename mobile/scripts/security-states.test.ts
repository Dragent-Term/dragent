import assert from "node:assert/strict";
import { test } from "node:test";
import {
  PAIR_AGAIN_MESSAGE as TRANSPORT_PAIR_AGAIN,
  UPDATE_APP_MESSAGE as TRANSPORT_UPDATE_APP,
} from "../src/transport/RelaySocketClient";
import {
  PAIR_AGAIN_MESSAGE,
  SECURITY_SCREEN_KINDS,
  UPDATE_APP_MESSAGE,
  securityStateFor,
  type SecurityScreenKind,
} from "../src/features/security/securityStates";
import { SEED_FAILED_MESSAGE } from "../src/transport/e2ee/desktopLink";

test("security wording mirrors the transport refusals (no drift)", () => {
  assert.equal(UPDATE_APP_MESSAGE, TRANSPORT_UPDATE_APP);
  assert.equal(PAIR_AGAIN_MESSAGE, TRANSPORT_PAIR_AGAIN);
  assert.equal(SEED_FAILED_MESSAGE, "Couldn't load this terminal's history");
});

test("pin-store loss fails closed before anything else", () => {
  const state = securityStateFor({
    secure: { state: "lost", problem: null },
    socketError: TRANSPORT_UPDATE_APP,
  });
  assert.equal(state?.kind, "store-lost");
  assert.match(state?.body ?? "", /couldn't be verified/);
  assert.deepEqual(state?.actions, ["pair-again"]);
});

test("admission refusals map to update-app and pair-again", () => {
  const update = securityStateFor({ socketError: TRANSPORT_UPDATE_APP });
  assert.equal(update?.kind, "update-app");
  assert.match(update?.title ?? "", /this phone/);
  const repair = securityStateFor({ socketError: TRANSPORT_PAIR_AGAIN });
  assert.equal(repair?.kind, "pair-again");
  assert.match(repair?.title ?? "", /end-to-end encrypted/);
});

test("refused handshake means pair again; unpaired means pair this desktop", () => {
  const refused = securityStateFor({
    secure: { state: "retrying", problem: "refused" },
    desktopName: "Studio",
  });
  assert.equal(refused?.kind, "pair-again");
  assert.match(refused?.title ?? "", /Studio/);
  const unpaired = securityStateFor({ secure: { state: "unpaired", problem: null } });
  assert.equal(unpaired?.kind, "pair-desktop");
  assert.equal(unpaired?.title, "Pair this desktop to open it.");
});

test("handshake failure after an open channel means the key changed", () => {
  const changed = securityStateFor({
    secure: { state: "retrying", problem: "handshake" },
    wasOpen: true,
    desktopName: "Studio",
  });
  assert.equal(changed?.kind, "key-changed");
  assert.match(changed?.title ?? "", /security key changed/);
  assert.match(changed?.body ?? "", /If you didn't, don't continue/);
  assert.deepEqual(changed?.actions, ["pair-again", "remove-desktop"]);

  const fresh = securityStateFor({
    secure: { state: "retrying", problem: "handshake" },
    wasOpen: false,
    desktopName: "Studio",
  });
  assert.equal(fresh?.kind, "cannot-connect");
  assert.match(fresh?.title ?? "", /Studio/);
  assert.match(fresh?.body ?? "", /Retrying/);
});

test("legacy desktop and interrupted channel map to their screens", () => {
  const legacy = securityStateFor({ desktopLegacy: true, desktopName: "Old Mac" });
  assert.equal(legacy?.kind, "update-desktop");
  assert.match(legacy?.title ?? "", /Old Mac/);
  const dropped = securityStateFor({
    secure: { state: "retrying", problem: "interrupted" },
    desktopName: "Studio",
  });
  assert.equal(dropped?.kind, "interrupted");
  assert.equal(dropped?.transient, true);
});

test("open, connecting and clean states show no screen", () => {
  assert.equal(securityStateFor({ secure: { state: "open", problem: null } }), null);
  assert.equal(securityStateFor({ secure: { state: "connecting", problem: null } }), null);
  assert.equal(securityStateFor({}), null);
});

test("every §9 phone screen kind has exact design wording", () => {
  const seen = new Set<SecurityScreenKind>();
  const cases: Array<{ kind: SecurityScreenKind; input: Parameters<typeof securityStateFor>[0] }> = [
    { kind: "store-lost", input: { secure: { state: "lost", problem: null } } },
    { kind: "update-app", input: { socketError: TRANSPORT_UPDATE_APP } },
    { kind: "update-desktop", input: { desktopLegacy: true, desktopName: "D" } },
    { kind: "pair-again", input: { socketError: TRANSPORT_PAIR_AGAIN } },
    { kind: "pair-desktop", input: { secure: { state: "unpaired", problem: null } } },
    { kind: "cannot-connect", input: { secure: { state: "retrying", problem: "handshake" }, desktopName: "D" } },
    {
      kind: "key-changed",
      input: { secure: { state: "retrying", problem: "handshake" }, wasOpen: true, desktopName: "D" },
    },
    { kind: "interrupted", input: { secure: { state: "retrying", problem: "interrupted" }, desktopName: "D" } },
  ];
  for (const { kind, input } of cases) {
    const state = securityStateFor(input);
    assert.equal(state?.kind, kind);
    seen.add(state!.kind);
  }
  assert.deepEqual([...seen].sort(), [...SECURITY_SCREEN_KINDS].sort());
});
