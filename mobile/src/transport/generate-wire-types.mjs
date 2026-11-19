/** Run after protocol changes; --check verifies the artifact without writing. */
import { readFileSync, writeFileSync } from "node:fs";

const source = readFileSync(new URL("../../../auth-portal/src/lib/sessionSocket.ts", import.meta.url), "utf8");
const start = source.indexOf('export type Role =');
const end = source.indexOf('export interface SessionSocketHandlers');
if (start < 0 || end <= start) throw new Error("Cannot locate canonical session wire types");
const header = "// Generated from auth-portal/src/lib/sessionSocket.ts. Do not edit by hand.\n" +
  "// Regenerate: node mobile/src/transport/generate-wire-types.mjs\n\n";
const expected = header + source.slice(start, end).trimEnd() + "\n";
const output = new URL("./wire-types.generated.ts", import.meta.url);
if (process.argv.includes("--check")) {
  let actual;
  try { actual = readFileSync(output, "utf8"); }
  catch (error) { if (error.code !== "ENOENT") throw error; }
  if (actual !== expected) {
    console.error("Session wire types are missing or outdated. Run pnpm protocol:sync in mobile/.");
    process.exitCode = 1;
  }
} else {
  writeFileSync(output, expected);
}
