/**
 * The live agent card (lock screen / Dynamic Island on iOS, an ongoing
 * notification on Android): every desktop's agents that are working or
 * waiting for you, most urgent first. Pure: built from the desktops' live
 * catalogs on the phone, never from the server.
 */
import type { RelayTab } from "../transport/relayCatalog";

/** Most urgent first: a question or failure, then a finished turn to read, then work. */
export type CardState = "needs_input" | "failed" | "review" | "working";

const URGENCY: Record<CardState, number> = { needs_input: 0, failed: 1, review: 2, working: 3 };

export interface CardRow {
  readonly key: string;
  readonly desktopId: string;
  readonly tabId: number;
  readonly title: string;
  /** Where it runs: "workspace · desktop" (or just the desktop). */
  readonly place: string;
  readonly state: CardState;
  /** When it entered this state (ms), as first seen on this phone. */
  readonly since: number;
}

export interface CardContent {
  readonly rows: readonly CardRow[];
  readonly needsYou: number;
  readonly working: number;
  readonly done: number;
  /** One line for the card's header and Android's collapsed text. */
  readonly summary: string;
}

/** Rows a card shows at most. */
export const CARD_ROWS = 4;

export function cardState(tab: RelayTab): CardState | null {
  if (tab.attention === "needs_input") return "needs_input";
  if (tab.attention === "failed") return "failed";
  if (tab.agent_state === "review") return "review";
  if (tab.agent_state === "working") return "working";
  return null;
}

export function stateLabel(state: CardState): string {
  switch (state) {
    case "needs_input":
      return "Needs you";
    case "failed":
      return "Failed";
    case "review":
      return "Done";
    case "working":
      return "Working";
  }
}

export function summaryOf(needsYou: number, done: number, working: number): string {
  const parts: string[] = [];
  if (needsYou) parts.push(`${needsYou} need${needsYou === 1 ? "s" : ""} you`);
  if (done) parts.push(`${done} done`);
  if (working) parts.push(`${working} working`);
  return parts.join(" · ") || "All quiet";
}

export function sortRows(rows: readonly CardRow[]): CardRow[] {
  return [...rows].sort((a, b) => URGENCY[a.state] - URGENCY[b.state] || b.since - a.since);
}

/** The card's content, or null when no agent is working or waiting (the card goes away). */
export function cardContent(rows: readonly CardRow[]): CardContent | null {
  if (rows.length === 0) return null;
  const sorted = sortRows(rows);
  const needsYou = sorted.filter((row) => row.state === "needs_input" || row.state === "failed").length;
  const done = sorted.filter((row) => row.state === "review").length;
  const working = sorted.filter((row) => row.state === "working").length;
  return Object.freeze({
    rows: Object.freeze(sorted.slice(0, CARD_ROWS)),
    needsYou,
    working,
    done,
    summary: summaryOf(needsYou, done, working),
  });
}
