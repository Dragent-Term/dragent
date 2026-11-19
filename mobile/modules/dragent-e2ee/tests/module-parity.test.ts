// The iOS (Swift) and Android (Kotlin) modules expose the same functions, and
// the TypeScript NativeE2ee interface describes exactly those (E4).
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";

const mod = join(import.meta.dirname, "..");
const swift = readFileSync(join(mod, "ios/DragentE2EEModule.swift"), "utf8");
const kotlin = readFileSync(join(mod, "android/src/main/java/dev/dragent/e2ee/DragentE2EEModule.kt"), "utf8");
const types = readFileSync(join(mod, "src/types.ts"), "utf8");

// Module-level definitions are indented 4 spaces, class members 6, in both files.
function names(src: string, indent: number): string[] {
  const re = new RegExp(`^ {${indent}}(?:Function|Property|Constant)\\("([A-Za-z0-9]+)"`, "gm");
  return [...src.matchAll(re)].map((m) => m[1]!).sort();
}

function classes(src: string): Map<string, string[]> {
  const out = new Map<string, string[]>();
  const re = /^ {4}Class\((?:"(\w+)", \w+::class|(\w+)\.self)\) \{\n([\s\S]*?)^ {4}\}/gm;
  for (const m of src.matchAll(re)) out.set((m[1] ?? m[2])!, names(m[3]!, 6));
  return out;
}

function tsInterface(name: string): string[] {
  const body = types.match(new RegExp(`export interface ${name} \\{([\\s\\S]*?)\\n\\}`))?.[1] ?? "";
  return [...body.matchAll(/^\s+(?:readonly )?(\w+)[(:]/gm)].map((m) => m[1]!).filter((n) => n !== "release").sort();
}

test("Swift and Kotlin modules define the same functions", () => {
  const s = names(swift, 4);
  assert.ok(s.length > 30, `parsed only ${s.length} Swift functions`);
  assert.deepEqual(names(kotlin, 4), s);
});

test("Swift and Kotlin classes define the same members", () => {
  const s = classes(swift);
  const k = classes(kotlin);
  assert.deepEqual([...k.keys()].sort(), [...s.keys()].sort());
  assert.equal(s.size, 6);
  for (const [name, members] of s) assert.deepEqual(k.get(name), members, name);
});

test("the TypeScript interface matches the native module", () => {
  assert.deepEqual(tsInterface("NativeE2ee"), names(swift, 4));
  const map: Record<string, string> = {
    ChannelRef: "Channel",
    KkHandshakeRef: "KkHandshake",
    PairingHandshakeRef: "PairingHandshake",
    ReassemblerRef: "Reassembler",
    PartSessionRef: "PartSession",
    PinStoreRef: "PinStore",
  };
  for (const [native, members] of classes(swift)) {
    assert.deepEqual(tsInterface(map[native]!), members, native);
  }
});
