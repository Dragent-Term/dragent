import type { AgentDotState } from "../components/AgentStateDot";
import type { RelayTab } from "../transport/relayCatalog";
import { countNeeds, statusDot } from "./attention";
import { machineWorkspacePath } from "./routes";

/**
 * Pure helpers for the live tab strip. Nothing here touches React Native so
 * `scripts/tab-strip.test.ts` can exercise it under plain node; the
 * strip's marks and screen-reader state words are applied in
 * `features/machines/WorkspaceTools.tsx`.
 */

export interface LiveTabStripItem {
  /** String id for `TabStrip` (live tab ids are numeric). */
  readonly id: string;
  readonly tabId: number;
  /** `title`, falling back to the kind label exactly like `LiveTabRow`. */
  readonly label: string;
  /** Kind label used for the accessible hint. */
  readonly kindLabel: string;
  /** Raw title (empty when the label is the fallback). */
  readonly title: string;
  readonly dot: AgentDotState;
  readonly kind: RelayTab["kind"];
  /** Trimmed explicit agent identity, or null when the host states none. */
  readonly agent: string | null;
}

/** Kind words for a live tab, mirroring `LiveTabRow`. */
export function liveTabKindLabel(tab: RelayTab): string {
  if (tab.kind === "agent") return tab.agent_kind ? `${tab.agent_kind} tab` : "Agent tab";
  if (tab.kind === "terminal") return tab.agent_kind ? `Terminal · ${tab.agent_kind}` : "Terminal tab";
  if (tab.kind === "browser") return "Browser tab";
  return "Settings tab";
}

/** Strip label: the tab's title, falling back to its kind like `LiveTabRow`. */
export function liveTabStripLabel(tab: RelayTab): string {
  return tab.title || liveTabKindLabel(tab);
}

/** Status dot for one tab: what it waits for, or idle when nothing is going on. */
export function liveTabDot(tab: RelayTab): AgentDotState {
  return statusDot(countNeeds([tab])) ?? "idle";
}

/** Strip options in catalog order; the open tab is selected by the caller. */
export function buildTabStripItems(tabs: readonly RelayTab[]): readonly LiveTabStripItem[] {
  return tabs.map((tab) => {
    const agent = (tab.agent_kind ?? "").trim();
    return {
      id: String(tab.id),
      tabId: tab.id,
      label: liveTabStripLabel(tab),
      kindLabel: liveTabKindLabel(tab),
      title: tab.title,
      dot: liveTabDot(tab),
      kind: tab.kind,
      agent: agent === "" ? null : agent,
    };
  });
}

/**
 * Route to the workspace screen with its "+ New tab" menu open: the
 * workspace screen opens that menu when `newTab=1` is set.
 */
export function workspaceNewTabRoute(deviceId: string, key: string): {
  pathname: "/machine/[deviceId]/workspace/[workspaceId]";
  params: { deviceId: string; workspaceId: string; newTab: string };
} {
  const base = machineWorkspacePath(deviceId, key);
  return {
    ...base,
    params: { ...base.params, newTab: "1" },
  };
}
