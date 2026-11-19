import type { AgentDotState } from "../components/AgentStateDot";
import type { RelayTab } from "../transport/relayCatalog";

/** Something on a desktop waiting for the person, most urgent first. */
export type Need = "needs_input" | "failed" | "review";

export interface NeedCounts {
  readonly needsInput: number;
  readonly failed: number;
  readonly review: number;
  /** Agents at work right now (not a need, but what a row shows meanwhile). */
  readonly working: number;
}

export const NO_NEEDS: NeedCounts = Object.freeze({ needsInput: 0, failed: 0, review: 0, working: 0 });

/** What one tab waits for: a permission prompt or question, a failure, or a finished turn to look at. */
export function tabNeed(tab: RelayTab): Need | null {
  if (tab.attention === "needs_input") return "needs_input";
  if (tab.attention === "failed") return "failed";
  if (tab.agent_state === "review") return "review";
  return null;
}

/** Counted on the phone, never the server: the catalog is the desktop's to read. */
export function countNeeds(tabs: readonly RelayTab[]): NeedCounts {
  let needsInput = 0;
  let failed = 0;
  let review = 0;
  let working = 0;
  for (const tab of tabs) {
    const need = tabNeed(tab);
    if (need === "needs_input") needsInput += 1;
    else if (need === "failed") failed += 1;
    else if (need === "review") review += 1;
    else if (tab.agent_state === "working") working += 1;
  }
  return Object.freeze({ needsInput, failed, review, working });
}

export function mostUrgent(counts: NeedCounts): Need | null {
  if (counts.needsInput > 0) return "needs_input";
  if (counts.failed > 0) return "failed";
  if (counts.review > 0) return "review";
  return null;
}

/**
 * The indicator on a project's, workspace's or tab's icon, in the desktop
 * sidebar's order: needs input, then failed, then the working spinner, then
 * ready for review. Null when nothing is going on.
 */
export function statusDot(counts: NeedCounts): AgentDotState | null {
  if (counts.needsInput > 0) return "waiting";
  if (counts.failed > 0) return "failed";
  if (counts.working > 0) return "working";
  if (counts.review > 0) return "done";
  return null;
}

/** The liveness dot for a need: red for input or a failure, green for review. */
export function needDot(need: Need): AgentDotState {
  if (need === "needs_input") return "waiting";
  if (need === "failed") return "failed";
  return "done";
}

/** `2 need input`, `1 failed`, `3 to review`. */
export function needLabel(need: Need, counts: NeedCounts): string {
  if (need === "needs_input") {
    return counts.needsInput === 1 ? "1 needs input" : `${counts.needsInput} need input`;
  }
  if (need === "failed") return `${counts.failed} failed`;
  return `${counts.review} to review`;
}

/** Every need, most urgent first, for a detail line or a screen reader. */
export function needsSummary(counts: NeedCounts): string | null {
  const parts = (["needs_input", "failed", "review"] as const)
    .filter((need) => (need === "needs_input" ? counts.needsInput : need === "failed" ? counts.failed : counts.review) > 0)
    .map((need) => needLabel(need, counts));
  return parts.length > 0 ? parts.join(" · ") : null;
}

/** An agent in these tabs is working, or stopped mid-task for the person's permission. */
export function isAtWork(counts: NeedCounts): boolean {
  return counts.working > 0 || counts.needsInput > 0;
}

/**
 * Workspaces at work first, everything else after, each part in the order it
 * came in (as the desktop sidebar orders a project). A workspace goes back
 * to its place when its agents stop.
 */
export function atWorkFirst<T>(items: readonly T[], countsOf: (item: T) => NeedCounts): T[] {
  const working: T[] = [];
  const rest: T[] = [];
  for (const item of items) (isAtWork(countsOf(item)) ? working : rest).push(item);
  return [...working, ...rest];
}
