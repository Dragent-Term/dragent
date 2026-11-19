/**
 * A workspace's tabs as the person thinks about them: what waits for them,
 * what is at work, and the rest. Pure, built from the catalog's tabs alone,
 * so the workspace screen and the tab strip agree on every tab's place.
 */
import type { RelayTab } from "../transport/relayCatalog";
import { tabNeed } from "./attention";

export type TabGroup = "needs_you" | "working" | "other";

export interface TabSection {
  readonly key: TabGroup;
  readonly title: string;
  readonly data: readonly RelayTab[];
}

const TITLES: Record<TabGroup, string> = {
  needs_you: "Needs you",
  working: "Working",
  other: "Other tabs",
};

/** Waiting on the person (input, a failure, or a finished turn to look at), at work, or neither. */
export function tabGroup(tab: RelayTab): TabGroup {
  if (tabNeed(tab) !== null) return "needs_you";
  if (tab.agent_state === "working") return "working";
  return "other";
}

/**
 * Sections in that order, empty ones left out. Within "Needs you" input
 * comes first, then failures, then reviews; otherwise the desktop's order.
 */
export function groupWorkspaceTabs(tabs: readonly RelayTab[]): readonly TabSection[] {
  const rank = (tab: RelayTab) => {
    const need = tabNeed(tab);
    return need === "needs_input" ? 0 : need === "failed" ? 1 : 2;
  };
  const by: Record<TabGroup, RelayTab[]> = { needs_you: [], working: [], other: [] };
  for (const tab of tabs) by[tabGroup(tab)].push(tab);
  by.needs_you = by.needs_you
    .map((tab, index) => ({ tab, index }))
    .sort((a, b) => rank(a.tab) - rank(b.tab) || a.index - b.index)
    .map(({ tab }) => tab);
  return (["needs_you", "working", "other"] as const)
    .filter((key) => by[key].length > 0)
    .map((key) => ({ key, title: TITLES[key], data: by[key] }));
}

/** How many of a workspace's tabs, other than the one open, wait for the person. */
export function othersNeedingYou(tabs: readonly RelayTab[], openTabId: number): number {
  return tabs.filter((tab) => tab.id !== openTabId && tabNeed(tab) !== null).length;
}

export interface StripLayout {
  readonly x: number;
  readonly width: number;
}

/**
 * Whether a tab that needs the person is scrolled out of sight to the left
 * or right of a horizontal strip, so the strip can show a dot at that edge.
 * A tab counts as hidden once less than `slack` points of it show.
 */
export function hiddenAttention(
  layouts: ReadonlyMap<string, StripLayout>,
  attention: ReadonlySet<string>,
  scrollX: number,
  viewport: number,
  slack = 12,
): { left: boolean; right: boolean } {
  let left = false;
  let right = false;
  if (viewport <= 0) return { left, right };
  for (const id of attention) {
    const at = layouts.get(id);
    if (!at) continue;
    if (at.x + at.width - slack < scrollX) left = true;
    else if (at.x + slack > scrollX + viewport) right = true;
  }
  return { left, right };
}
