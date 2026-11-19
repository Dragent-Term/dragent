// §6.6 / §8 phone cases (E9a): the push-key announcement and the reconcile
// request ride the real KK channel (Rust binding) to a scripted desktop
// (real dragent_e2ee), which decodes them with the desktop's own codec.
import assert from "node:assert/strict";
import { test } from "node:test";

import { attachPushChannel, PushKeySync } from "../../../notifications/e2eePush";
import { ACCOUNT, DESKTOP, flush, PHONE, world, type World } from "./fixture";

async function open(w: World): Promise<void> {
  await w.admit();
  w.pump();
  w.pump();
  assert.equal(w.client.getSnapshot().secure?.state, "open");
}

function lastCtr(w: World): { text: string; ctrlCanonical?: boolean } {
  const m = [...w.inbox].reverse().find((x) => "mtype" in x && x.mtype === "ctrl");
  assert.ok(m && "mtype" in m);
  return m as { text: string; ctrlCanonical?: boolean };
}

test("announcePushKey sends identity-rotate with the phone's CURRENT identity key and the new push key", async () => {
  const w = world();
  try {
    const before = w.native.identityPublicKey()!;
    const newPush = new Uint8Array(32).fill(7);
    assert.equal(w.client.announcePushKey(newPush), false, "nothing before the channel is open");
    await open(w);
    assert.equal(w.client.announcePushKey(new Uint8Array(31)), false, "a malformed key is refused");
    assert.equal(w.client.announcePushKey(newPush), true);
    w.pump();
    const m = lastCtr(w);
    assert.ok(m.ctrlCanonical, "the desktop's codec accepts it");
    assert.ok(m.text.startsWith("identity-rotate "));
    const body = JSON.parse(m.text.slice("identity-rotate ".length)) as { new_pub: string; new_push_pub: string };
    assert.deepEqual([...Buffer.from(body.new_pub, "base64url")], [...before], "the identity pin is kept");
    assert.deepEqual([...Buffer.from(body.new_push_pub, "base64url")], [...newPush]);
    assert.deepEqual([...w.native.identityPublicKey()!], [...before], "the identity key never changes");
    assert.ok(w.socket().text.every((f) => f.type !== "identity-rotate"), "never as text");
  } finally {
    w.close();
  }
});

test("on channel open the phone announces a pending push key, then asks notifications.done inside the channel", async () => {
  const w = world();
  try {
    const prefs = new Map<string, string>();
    let now = 0;
    const native = { ...w.native, rotatePushKey: () => w.native.createPushKey() };
    const sync = new PushKeySync({
      native,
      prefs: { getItem: async (k) => prefs.get(k) ?? null, setItem: async (k, v) => { prefs.set(k, v); }, removeItem: async (k) => { prefs.delete(k); } },
      account: ACCOUNT,
      phone: PHONE,
      pairedDesktops: () => [DESKTOP],
      now: () => now,
    });
    await sync.maybeRotate();
    now = 181 * 24 * 60 * 60 * 1000;
    const removed: string[] = [];
    const stop = attachPushChannel(w.client, DESKTOP, { sync, platform: "ios", dismiss: async (ids) => { removed.push(...ids); }, now: () => now });
    await open(w);
    await flush();
    await new Promise((r) => setTimeout(r, 20));
    w.pump();
    const rotate = w.inbox.find((x) => "mtype" in x && x.mtype === "ctrl" && x.text.startsWith("identity-rotate "));
    assert.ok(rotate && "ctrlCanonical" in rotate && rotate.ctrlCanonical);
    const pushNow = w.native.pushPublicKey()!;
    assert.ok(rotate.text.includes(Buffer.from(pushNow).toString("base64url")), "announces the rotated key");
    const req = w.requests().find((r) => r.method === "notifications.done");
    assert.ok(req);
    assert.deepEqual(req.params, { platform: "ios" });
    w.reply(req.id as string, { ids: ["AbCdEfGhIjKlMnOpQrSt_-"], tags: [] });
    await new Promise((r) => setTimeout(r, 20));
    assert.deepEqual(removed, ["AbCdEfGhIjKlMnOpQrSt_-"]);
    stop();
  } finally {
    w.close();
  }
});
