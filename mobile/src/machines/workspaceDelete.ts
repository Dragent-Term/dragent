/** What a workspace delete the desktop held back would lose, in words. */
import type { WorkspaceDeleteResult } from "../transport/relayRpcMethods";

/** "Claude Code (a1b2) is still working in it. 3 uncommitted changes would be lost." */
export function blockedMessage(result: Extract<WorkspaceDeleteResult, { deleted: false }>): string {
  const parts: string[] = [];
  if (result.agents.length > 0) {
    const who = result.agents.length === 1 ? result.agents[0] : `${result.agents.length} agents`;
    parts.push(`${who} ${result.agents.length === 1 ? "is" : "are"} still working in it.`);
  }
  if (result.changes > 0) {
    parts.push(
      `${result.changes} uncommitted ${result.changes === 1 ? "change" : "changes"} would be lost.`,
    );
  }
  return parts.join(" ");
}

