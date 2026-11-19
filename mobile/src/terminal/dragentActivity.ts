import type { RelayAgentActivity } from "../transport/relayCatalog";
import type { TerminalAgentBlock } from "./types";

/**
 * Phone-side projection of the Dragent pipeline entries the relay catalog
 * carries (`agent_activity`) onto the terminal conversation row vocabulary,
 * so the mobile feed reuses the `AgentBlock` style instead of inventing a
 * new one. Pure data mapping only: no React, no theme, no I/O — and no
 * credential or prompt writes anywhere near it.
 */

/** A pipeline kind onto a conversation row kind; unknown kinds stay plain updates. */
export function dragentActivityBlockKind(kind: string): TerminalAgentBlock["kind"] {
  switch (kind.trim().toLowerCase()) {
    case "thinking":
      return "thinking";
    case "tool":
      return "tool_step";
    case "narration":
    case "finished":
      return "agent_message";
    default:
      return "unknown";
  }
}

/** Row label for a pipeline kind; unknown kinds show verbatim (or `Activity`). */
export function dragentActivityBlockLabel(kind: string): string {
  switch (kind.trim().toLowerCase()) {
    case "thinking":
      return "Thinking";
    case "tool":
      return "Tool";
    case "narration":
      return "Update";
    case "finished":
      return "Finished";
    default: {
      const clean = kind.trim();
      return clean === "" ? "Activity" : clean;
    }
  }
}

/**
 * Feed rows for the catalog entries, oldest-first (newest last), in wire
 * order. Text is already bounded server-side (`MAX_AGENT_ACTIVITY_TEXT_CHARS`).
 * Rows keep the pipeline sequence as their id so a reader's expand/collapse
 * choice survives catalog updates while the feed grows; the wire carries no
 * block ids or timestamps, so `tsMs` stays unset.
 */
export function dragentActivityBlocks(
  activity: readonly RelayAgentActivity[],
): readonly TerminalAgentBlock[] {
  return Object.freeze(
    activity.map((entry) =>
      Object.freeze({
        id: entry.sequence,
        tsMs: 0,
        kind: dragentActivityBlockKind(entry.kind),
        label: dragentActivityBlockLabel(entry.kind),
        text: entry.text,
        status: null,
        truncated: false,
      }),
    ),
  );
}
