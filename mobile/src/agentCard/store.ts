/**
 * What the agent card shows, fed by every desktop the app is connected to
 * (each machine connection publishes its live catalog here). Remembers when
 * each tab entered its state, so the card can say "for 12 min".
 */
import type { RelayCatalog, RelayTab } from "../transport/relayCatalog";
import { workspaceKey } from "../machines/machineCatalog";
import { cardContent, cardState, type CardContent, type CardRow, type CardState } from "./model";

interface DesktopRows {
  readonly name: string;
  readonly rows: readonly CardRow[];
}

const desktops = new Map<string, DesktopRows>();
/** tab key -> [state, since] */
const seen = new Map<string, [CardState, number]>();
const listeners = new Set<() => void>();
let content: CardContent | null = null;

function rebuild() {
  content = cardContent([...desktops.values()].flatMap((desktop) => desktop.rows));
  for (const listener of listeners) listener();
}

function title(tab: RelayTab): string {
  return (tab.title || tab.agent_kind || "Agent").slice(0, 80);
}

/** A desktop's latest catalog. `null` when it went away (offline, unpaired). */
export function publishDesktop(desktopId: string, desktopName: string, catalog: RelayCatalog | null, now = Date.now()): void {
  if (!catalog) {
    if (desktops.delete(desktopId)) rebuild();
    return;
  }
  const workspaces = new Map<string, string>();
  for (const workspace of catalog.workspaces) workspaces.set(workspaceKey(workspace), workspace.name);
  const rows: CardRow[] = [];
  for (const tab of catalog.tabs) {
    const state = cardState(tab);
    const key = `${desktopId}:${tab.id}`;
    if (!state) {
      seen.delete(key);
      continue;
    }
    const before = seen.get(key);
    const since = before && before[0] === state ? before[1] : now;
    seen.set(key, [state, since]);
    const workspace = tab.workspace_id ? workspaces.get(tab.workspace_id) : undefined;
    rows.push({
      key,
      desktopId,
      tabId: tab.id,
      title: title(tab),
      place: workspace ? `${workspace} · ${desktopName}` : desktopName,
      state,
      since,
    });
  }
  const previous = desktops.get(desktopId);
  if (previous && JSON.stringify(previous.rows) === JSON.stringify(rows) && previous.name === desktopName) return;
  desktops.set(desktopId, { name: desktopName, rows });
  rebuild();
}

export function getCard(): CardContent | null {
  return content;
}

export function subscribeCard(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Tests only. */
export function resetCardStore(): void {
  desktops.clear();
  seen.clear();
  content = null;
}
