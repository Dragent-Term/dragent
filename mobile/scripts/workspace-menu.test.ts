import assert from "node:assert/strict";
import { test } from "node:test";
import { codeLanguage, highlightCode, highlightLine } from "../src/features/workspaceMenu/codeHighlight";
import { parseUnifiedDiff } from "../src/features/workspaceMenu/unifiedDiff";
import { baseName, crumbs, joinPath, parentPath, relativeTo, sizeLabel, sortEntries } from "../src/features/workspaceMenu/paths";
import {
  listFolder,
  openFilePage,
  parseGitStatus,
  readGitDiff,
  readGitStatus,
  searchFolder,
} from "../src/transport/relayRpcMethods";
import type { RelayRpcClient } from "../src/transport/relayRpcClient";

const kinds = (line: string, language: string) =>
  highlightLine(line, language, null).tokens.map((token) => `${token.kind}:${token.text}`);

test("code is coloured by what each piece is", () => {
  assert.deepEqual(kinds('const n = 42; // why', "c"), [
    "keyword:const", "plain: n = ", "number:42", "plain:; ", "comment:// why",
  ]);
  assert.deepEqual(kinds('let s: String = "a // b";', "rust"), [
    "keyword:let", "plain: s: ", "type:String", "plain: = ", 'string:"', 'string:a // b"', "plain:;",
  ]);
  assert.deepEqual(kinds("x = 1  # note", "python"), ["plain:x = ", "number:1", "plain:  ", "comment:# note"]);
  // An identifier that only contains a digit or a keyword is not one.
  assert.deepEqual(kinds("format2 iffy", "c"), ["plain:format2 iffy"]);
  assert.deepEqual(kinds("anything { at } all", "plain"), ["plain:anything { at } all"]);
});

test("block comments and multi-line strings carry over lines; one-line strings don't", () => {
  const lines = highlightCode("a /* one\ntwo */ b\nconst s = `x\ny`;\nlet q = 'open\nnext", "c");
  assert.deepEqual(lines[0]!.map((t) => t.kind), ["plain", "comment", "comment"]);
  assert.deepEqual(lines[1]!.map((t) => `${t.kind}:${t.text}`), ["comment:two */", "plain: b"]);
  assert.equal(lines[2]!.at(-1)!.kind, "string");
  assert.deepEqual(lines[3]!.map((t) => `${t.kind}:${t.text}`), ["string:y`", "plain:;"]);
  // An unclosed quote ends with its line.
  assert.equal(lines[5]!.every((t) => t.kind === "plain"), true);
  // An escaped quote doesn't close the string.
  assert.deepEqual(kinds('"a\\"b" c', "c").slice(-1), ["plain: c"]);
});

test("the language comes from the file name", () => {
  assert.equal(codeLanguage("/w/src/App.tsx"), "c");
  assert.equal(codeLanguage("main.rs"), "rust");
  assert.equal(codeLanguage("Dockerfile"), "shell");
  assert.equal(codeLanguage("config.YAML"), "data");
  assert.equal(codeLanguage("README"), "plain");
});

test("a unified diff becomes numbered rows", () => {
  const diff = [
    "diff --git a/a.txt b/a.txt", "index 1..2 100644", "--- a/a.txt", "+++ b/a.txt",
    "@@ -3,4 +3,5 @@ fn main() {", " keep", "-old", "+new", "+more", " tail", "\\ No newline at end of file", "",
  ].join("\n");
  const parsed = parseUnifiedDiff(diff);
  assert.deepEqual([parsed.added, parsed.removed], [2, 1]);
  assert.deepEqual(parsed.rows.map((row) => [row.kind, row.oldLine, row.newLine, row.text]), [
    ["hunk", null, null, "@@ -3,4 +3,5 @@ fn main() {"],
    ["ctx", 3, 3, "keep"],
    ["del", 4, null, "old"],
    ["add", null, 4, "new"],
    ["add", null, 5, "more"],
    ["ctx", 5, 6, "tail"],
    ["note", null, null, "No newline at end of file"],
  ]);
  // Header lines that look like changes are not counted.
  assert.equal(parseUnifiedDiff("--- a/x\n+++ b/x\n").rows.length, 0);
  assert.equal(parseUnifiedDiff("").rows.length, 0);
});

test("paths stay inside the workspace", () => {
  assert.equal(joinPath("/w", "src"), "/w/src");
  assert.equal(joinPath("/", "etc"), "/etc");
  assert.equal(parentPath("/w/src/a.ts"), "/w/src");
  assert.equal(parentPath("/w"), "/");
  assert.equal(baseName("/w/src/a.ts"), "a.ts");
  assert.equal(relativeTo("/w", "/w/src/a.ts"), "src/a.ts");
  assert.equal(relativeTo("/w", "/w"), "");
  assert.equal(relativeTo("/w", "/wider/a"), null);
  assert.deepEqual(crumbs("/home/me/app", "/home/me/app/src/ui"), [
    { label: "app", path: "/home/me/app" },
    { label: "src", path: "/home/me/app/src" },
    { label: "ui", path: "/home/me/app/src/ui" },
  ]);
  const sorted = sortEntries([
    { name: "b.ts", kind: "file" }, { name: "Zed", kind: "directory" }, { name: "a10.ts", kind: "file" },
    { name: "a2.ts", kind: "file" }, { name: "app", kind: "directory" },
  ]);
  assert.deepEqual(sorted.map((entry) => entry.name), ["app", "Zed", "a2.ts", "a10.ts", "b.ts"]);
  assert.deepEqual([sizeLabel(640), sizeLabel(2048), sizeLabel(5 * 1024 * 1024)], ["640 B", "2.0 KB", "5.0 MB"]);
});

function fakeClient(calls: Array<{ method: string; params: unknown }>, reply: unknown): RelayRpcClient {
  return {
    request: async (method: string, params: unknown) => {
      calls.push({ method, params });
      return reply;
    },
  } as unknown as RelayRpcClient;
}

test("file and git requests are shaped as the desktop expects and checked first", async () => {
  const calls: Array<{ method: string; params: unknown }> = [];
  const entries = { entries: [{ name: "a.ts", kind: "file", path: "/w/src/a.ts" }, { name: "src", kind: "directory" }] };
  const listed = await listFolder(fakeClient(calls, entries), "/w");
  assert.deepEqual(listed[0], { name: "a.ts", kind: "file", path: "/w/src/a.ts" });
  assert.deepEqual(listed[1], { name: "src", kind: "directory" });
  await searchFolder(fakeClient(calls, entries), "/w", " app ");
  const page = await openFilePage(fakeClient(calls, { content: "x", size: 9, offset: 0, next_offset: 4, truncated: true }), "/w/a.ts", 0);
  assert.deepEqual(page, { content: "x", size: 9, nextOffset: 4 });
  await readGitDiff(fakeClient(calls, { diff: "", binary: true, truncated: false }), "/w", "src/a.ts", "staged");
  assert.deepEqual(calls, [
    { method: "files.list", params: { path: "/w", all: true } },
    { method: "files.search", params: { query: "app", base: "/w", limit: 200 } },
    { method: "files.open", params: { path: "/w/a.ts", offset: 0 } },
    { method: "git.diff", params: { path: "/w", file: "src/a.ts", side: "staged" } },
  ]);
  calls.length = 0;
  const client = fakeClient(calls, {});
  await assert.rejects(readGitDiff(client, "/w", "../secrets", "unstaged"));
  await assert.rejects(readGitDiff(client, "/w", "/etc/passwd", "unstaged"));
  await assert.rejects(openFilePage(client, "/w/a.ts", -1));
  await assert.rejects(searchFolder(client, "/w", "  "));
  assert.equal(calls.length, 0);
  // A last page has no next offset; a malformed page is an error.
  await assert.rejects(openFilePage(fakeClient([], { content: "x" }), "/w/a.ts", 0));
  await assert.rejects(readGitStatus(fakeClient([], { staged: "no" }), "/w"));
});

test("git status keeps what it understands and bounds the rest", () => {
  const status = parseGitStatus({
    repository: true, branch: "main", upstream: "origin/main", ahead: 2, behind: 0, truncated: false,
    staged: [{ path: "new.rs", old_path: "old.rs", status: "renamed", added: 3, removed: 1 }],
    unstaged: [{ path: "pic.png", old_path: null, status: "odd", added: null, removed: null }],
  });
  assert.ok(status);
  assert.deepEqual(status.staged[0], { path: "new.rs", oldPath: "old.rs", status: "renamed", added: 3, removed: 1 });
  assert.deepEqual(status.unstaged[0], { path: "pic.png", oldPath: null, status: "modified", added: null, removed: null });
  assert.equal(parseGitStatus({ repository: false, staged: [], unstaged: [] })?.repository, false);
  assert.equal(parseGitStatus({ staged: [{ path: "" }], unstaged: [] }), null);
});

import { browserAct, parseBrowserActResult } from "../src/transport/relayRpcMethods";

test("browser actions carry values, never script, and results are checked", async () => {
  const calls: Array<{ method: string; params: unknown }> = [];
  const page = { page: true, url: "http://x/", title: "X", width: 1200, height: 800, top: 0, full: 2400 };
  const client = fakeClient(calls, page);
  await browserAct(client, 9, "view", { claim: true });
  await browserAct(client, 9, "open", { url: " localhost:3000 " });
  await browserAct(client, 9, "tap", { x: 0.25, y: 0.5 });
  await browserAct(client, 9, "scroll", { by: -0.8 });
  await browserAct(client, 9, "type", { text: "hi" });
  await browserAct(client, 9, "press", { key: "Enter" });
  await browserAct(client, 9, "back");
  assert.deepEqual(calls.map((call) => call.params), [
    { tab_id: 9, action: "view", claim: true },
    { tab_id: 9, action: "open", url: "localhost:3000" },
    { tab_id: 9, action: "tap", x: 0.25, y: 0.5 },
    { tab_id: 9, action: "scroll", by: -0.8 },
    { tab_id: 9, action: "type", text: "hi" },
    { tab_id: 9, action: "press", key: "Enter" },
    { tab_id: 9, action: "back" },
  ]);
  calls.length = 0;
  // Mobile view sends this phone's page area; desktop view sends none.
  await browserAct(client, 9, "view", { mobile: { width: 390.4, height: 663.6 } });
  await browserAct(client, 9, "view", { mobile: null });
  assert.deepEqual(calls.map((call) => call.params), [
    { tab_id: 9, action: "view", mobile: { width: 390, height: 664 } },
    { tab_id: 9, action: "view" },
  ]);
  await assert.rejects(browserAct(client, 9, "view", { mobile: { width: 20, height: 600 } }));
  calls.length = 0;
  await assert.rejects(browserAct(client, 9, "tap", { x: 2, y: 0 }));
  await assert.rejects(browserAct(client, 9, "open", { url: " " }));
  await assert.rejects(browserAct(client, 9, "press", { key: "F12" as never }));
  await assert.rejects(browserAct(client, 9, "scroll", { by: 99 }));
  assert.equal(calls.length, 0);
  assert.deepEqual(parseBrowserActResult({ done: true }), { kind: "done" });
  assert.deepEqual(parseBrowserActResult({ page: false, message: "No page is open." }), { kind: "no_page", message: "No page is open." });
  // The page comes back with its picture in the same answer.
  assert.deepEqual(parseBrowserActResult({ ...page, image: "QUJD", image_width: 10, image_height: 5 }), {
    kind: "page", url: "http://x/", title: "X", width: 1200, height: 800, top: 0, full: 2400,
    picture: { image: "QUJD", width: 10, height: 5 }, background: null, overlays: [],
  });
  // What the page keeps in place (a sticky header) comes apart from its picture.
  const pinned = parseBrowserActResult({
    ...page, bg: "rgb(5, 7, 9)", overlays: [{ x: 0, y: 0, width: 390, height: 60, image: "QUJD" }],
  });
  assert.ok(pinned?.kind === "page");
  assert.equal(pinned.background, "rgb(5, 7, 9)");
  assert.deepEqual(pinned.overlays, [{ x: 0, y: 0, width: 390, height: 60, image: "QUJD" }]);
  assert.equal((parseBrowserActResult({ ...page, bg: "url(javascript:1)" }) as { background: unknown }).background, null);
  assert.equal(parseBrowserActResult({ ...page, overlays: [{ x: 0, y: 0, width: 0, height: 60, image: "QUJD" }] }), null);
  assert.equal((parseBrowserActResult(page) as { picture: unknown }).picture, null);
  // A picture that isn't base64, or has no size, is refused.
  assert.equal(parseBrowserActResult({ ...page, image: "<svg onload=x>", image_width: 10, image_height: 5 }), null);
  assert.equal(parseBrowserActResult({ ...page, image: "QUJD", image_width: 0, image_height: 5 }), null);
  assert.equal(parseBrowserActResult({}), null);
});

import { addTile, clampTop, covers, MAX_TILES, nextPeek, windowScrolls, type PageTile } from "../src/features/browserTab/pageTiles";

const tile = (top: number, seq: number, height = 700): PageTile => ({ top, height, uri: `data:${seq}`, seq });

test("the page is remembered as pictures at their place on the page", () => {
  let tiles: PageTile[] = [];
  tiles = addTile(tiles, tile(0, 1), 0);
  tiles = addTile(tiles, tile(700, 2), 0);
  assert.equal(covers(tiles, 0, 1400), true);
  assert.equal(covers(tiles, 0, 1500), false);
  assert.equal(covers([tile(0, 1), tile(900, 2)], 0, 1600), false, "a gap is not covered");
  // A newer picture of the same place keeps one older one under it, no more.
  tiles = addTile(tiles, tile(0, 3), 0);
  tiles = addTile(tiles, tile(1, 4), 0);
  assert.deepEqual(tiles.filter((t) => t.top <= 2).map((t) => t.seq).sort(), [3, 4]);
  // Beyond the limit, the tiles furthest from the reader go first.
  let many: PageTile[] = [];
  for (let index = 0; index < MAX_TILES + 4; index += 1) many = addTile(many, tile(index * 700, index), 0);
  assert.equal(many.length, MAX_TILES);
  assert.ok(many.some((t) => t.top === 0), "the tile under the reader stays");
  assert.ok(many.some((t) => t.seq === MAX_TILES + 3), "the newest tile always stays");
});

test("the page around the reader is fetched before they get there", () => {
  const full = 3000;
  // Only the visible screen is known: look below first.
  assert.equal(nextPeek([tile(0, 1)], 0, 700, full), 700);
  // Then further down, a screen at a time, until the page ends.
  assert.equal(nextPeek([tile(0, 1), tile(700, 2)], 0, 700, full), 1400);
  assert.equal(nextPeek([tile(0, 1), tile(700, 2), tile(1400, 3), tile(2100, 4)], 0, 700, full), 2300);
  assert.equal(nextPeek([tile(0, 1), tile(700, 2), tile(1400, 3), tile(2100, 4), tile(2300, 5)], 0, 700, full), null);
  // Moving up, the screen above comes first.
  assert.equal(nextPeek([tile(1400, 1)], 1400, 700, full, -1), 700);
  assert.equal(nextPeek([tile(1400, 1)], 1400, 700, full, 1), 2100);
  // At the bottom, only what is above is left.
  assert.equal(nextPeek([tile(2300, 1)], 2300, 700, full, 1), 1600);
  assert.equal(clampTop(5000, full, 700), 2300);
  assert.equal(clampTop(-20, full, 700), 0);
  // A page that fits its screen doesn't scroll at the window.
  assert.equal(windowScrolls(700, 700), false);
  assert.equal(windowScrolls(3000, 700), true);
  assert.equal(nextPeek([tile(0, 1)], 0, 700, 700), null);
});
