import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { normalizeAgentKind } from "../src/components/agentArtwork";
import { AGENTS } from "./agent-marks-table.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const desktop = path.resolve(here, "../../dragent/app/src/workspace/action.rs");
const images = path.resolve(here, "../../dragent/app/assets/dragent/agent-images");
const generated = path.resolve(here, "../src/components/agentMarks.generated.ts");

/** Every agent's command name, from the desktop's `PromptAgent::cli`. */
function desktopAgents(): string[] {
  const source = readFileSync(desktop, "utf8");
  const start = source.indexOf("pub fn cli(self)");
  const body = source.slice(start, source.indexOf("pub fn command_key", start));
  return [...body.matchAll(/=> Some\("([^"]+)"\)/g)].map((match) => match[1]!);
}

test("every agent the desktop knows has its icon on the phone", () => {
  const agents = desktopAgents();
  assert.ok(agents.length >= 30, `read ${agents.length} agents from action.rs`);
  const table = new Set((AGENTS as [string, string, string, string | null][]).map(([id]) => id));
  const missing = agents.filter((id) => normalizeAgentKind(id) === null && !table.has(id));
  assert.deepEqual(missing, [], "agents without an icon on the phone: run the sync script after adding them");
});

test("each icon comes from a desktop file that exists, and the generated table is current", () => {
  const output = readFileSync(generated, "utf8");
  for (const [id, , light, dark] of AGENTS as [string, string, string, string | null][]) {
    for (const file of [light, dark].filter((file): file is string => file !== null)) {
      assert.ok(existsSync(path.join(images, file)), `${id}: ${file}`);
    }
    assert.ok(output.includes(`${JSON.stringify(id)}: {`), `${id} missing from agentMarks.generated.ts`);
  }
});
