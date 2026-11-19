/**
 * Agent logos are their makers' own artwork. The single-colour ones disappear
 * on one of the two themes, so each is tagged with the ground it fades into.
 */

// Black (or near-black) artwork: lightened on dark grounds.
const DARK_MARKS = new Set([
  "auggie",
  "autohand",
  "codex",
  "copilot",
  "cursor",
  "devin",
  "droid",
  "kimi",
  "letta",
  "mimocode",
  "ohmpi",
  "pi",
  "primeagent",
]);

// White artwork: darkened on light grounds.
const LIGHT_MARKS = new Set(["grok"]);

/** The class that keeps an agent's logo visible on both themes, if it needs one. */
export function agentMarkClass(id: string): string | undefined {
  if (DARK_MARKS.has(id)) return "agent-mark-dark";
  if (LIGHT_MARKS.has(id)) return "agent-mark-light";
  return undefined;
}
