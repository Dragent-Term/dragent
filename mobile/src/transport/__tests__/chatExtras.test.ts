import assert from "node:assert/strict";
import { test } from "node:test";
import { completeMention, mentionAtEnd, modelKey, readChatModels, uploadFile, uploadPieces, withAttachments } from "../../chat/extras";
import type { RelayRpcClient } from "../relayRpcClient";

test("a file is cut into pieces that cover it exactly", () => {
  assert.deepEqual(uploadPieces(10, 4), [{ offset: 0, length: 4 }, { offset: 4, length: 4 }, { offset: 8, length: 2 }]);
  assert.deepEqual(uploadPieces(0), []);
});

test("an upload sends every piece, then ends with the saved path", async () => {
  const calls: { method: string; params: Record<string, unknown> }[] = [];
  const client = {
    request: async (method: string, params: Record<string, unknown>) => {
      calls.push({ method, params });
      if (method === "files.upload_begin") return { upload: "u1" };
      if (method === "files.upload_chunk") return { received: 1 };
      return { path: "/home/me/.dragent/phone-uploads/2026-11-16/shot.png", name: "shot.png", size: 100_000 };
    },
  } as unknown as RelayRpcClient;
  const progress: number[] = [];
  const saved = await uploadFile(client, { name: "shot.png", size: 100_000 }, async (offset, length) => `${offset}:${length}`, (share) => progress.push(share));
  assert.equal(saved.name, "shot.png");
  const pieces = calls.filter((call) => call.method === "files.upload_chunk").map((call) => call.params.offset).sort((a, b) => Number(a) - Number(b));
  assert.deepEqual(pieces, [0, 46080, 92160]);
  assert.equal(calls.at(-1)!.method, "files.upload_end");
  assert.equal(progress.at(-1), 1);
});

test("attached files are named after the message, quoted", () => {
  assert.equal(withAttachments("look", ["/a/b.png"]), "look\n\n'/a/b.png'");
  assert.equal(withAttachments("", ["/a/it's.png"]), "'/a/it'\\''s.png'");
  assert.equal(withAttachments("plain", []), "plain");
});

test("@ references complete relative to the workspace", () => {
  assert.deepEqual(mentionAtEnd("fix @src/ap"), { query: "src/ap", start: 4 });
  assert.equal(mentionAtEnd("mail me@x.com later"), null);
  assert.equal(completeMention("fix @ap", "/w/src/app.ts", "/w"), "fix @src/app.ts ");
  assert.equal(completeMention("@", "/elsewhere/x.ts", "/w"), "@/elsewhere/x.ts ");
});

test("the model menu reads, and malformed menus are refused", () => {
  const menu = readChatModels({
    available: true,
    current: { id: "auto", effort: null },
    choices: [{ id: "auto", label: "Auto", effort: null }, { id: "gpt-5", label: "GPT-5 (high)", effort: "high" }],
  });
  assert.ok(menu);
  assert.equal(menu.choices[1]!.effort, "high");
  assert.notEqual(modelKey("gpt-5", "high"), modelKey("gpt-5", null));
  assert.equal(readChatModels({ available: true, choices: [{ id: 3 }] }), null);
});
