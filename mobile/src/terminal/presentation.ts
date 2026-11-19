import type { TextStyle } from "react-native";
import type { AgentDotState } from "../components/AgentStateDot";
import type { StatusTone } from "../components/StatusBadge";
import type { WireColor } from "../transport";
import type { TerminalAgentBlock, TerminalAltScreen, TerminalBlock, TerminalSpan, TerminalState } from "./types";

export type TerminalItem =
  | { key: string; kind: "command"; block: TerminalBlock; tsMs: number }
  | { key: string; kind: "agent"; block: TerminalAgentBlock; tsMs: number }
  | { key: string; kind: "alt"; screen: TerminalAltScreen; tsMs: number };

export type TerminalContent = "all" | "terminal" | "agent";

/** Which conversation role an agent row represents for restrained visual hierarchy. */
export type AgentRole =
  | "user"
  | "agent"
  | "system"
  | "thought"
  | "tool"
  | "result"
  | "request"
  | "status"
  | "group"
  | "unknown";

export function agentRole(kind: TerminalAgentBlock["kind"]): AgentRole {
  switch (kind) {
    case "user_query": return "user";
    case "agent_message":
    case "error": return "agent";
    case "system_note": return "system";
    case "thinking": return "thought";
    case "tool_step": return "tool";
    case "action_result": return "result";
    case "approval":
    case "question":
    case "plan_review": return "request";
    case "queued":
    case "loading": return "status";
    case "turn_group":
    case "activity_group": return "group";
    default: return "unknown";
  }
}

/** Verbose rows get a disclosure so long thinking/tool output stays skimmable. */
export function isAgentCollapsible(kind: TerminalAgentBlock["kind"]): boolean {
  return kind === "thinking" || kind === "tool_step" || kind === "turn_group" || kind === "activity_group";
}

/** Verbose rows start collapsed; everything else starts expanded. */
export function defaultAgentExpanded(kind: TerminalAgentBlock["kind"]): boolean {
  return !isAgentCollapsible(kind);
}

/**
 * Stable collapse key for one agent row. Keyed by block id so a reader's
 * expand/collapse choice survives re-renders, history trims, and incoming
 * deltas; it matches the `terminalItems` agent key for the same block.
 */
export function agentCollapseKey(id: number): string {
  return `agent:${id}`;
}

/** Resolve controlled collapse state with collapsed-by-default for verbose rows. */
export function resolveAgentExpanded(
  kind: TerminalAgentBlock["kind"],
  id: number,
  expanded: Readonly<Record<string, boolean>>,
): boolean {
  return expanded[agentCollapseKey(id)] ?? defaultAgentExpanded(kind);
}

/**
 * One AgentStateDot vocabulary for every agent row, so stop/retry/interrupted
 * read the same as elsewhere in the app. Only the host status (plus the
 * established stop-note pattern) is read; prose is never parsed for
 * completion. Falls back to idle so quiet rows stay restrained.
 */
export function agentDotState(block: Pick<TerminalAgentBlock, "kind" | "status" | "text">): AgentDotState {
  const status = (block.status ?? "").toLowerCase();
  const text = (block.text ?? "").toLowerCase();
  if (block.kind === "error") return "failed";
  if (/(fail|error|denied|reject)/.test(status)) return "failed";
  if (/(stopp|cancel|interrupt|abort)/.test(status)) return "interrupted";
  if (block.kind === "system_note" && /(stopp|cancel|interrupt|abort)/.test(text)) return "interrupted";
  if (block.kind === "approval" || block.kind === "question" || block.kind === "plan_review") {
    if (/(done|complete|success|approv|confirm|accept)/.test(status)) return "done";
    return "blocked";
  }
  if (block.kind === "loading") return "working";
  if (block.kind === "queued") return "waiting";
  if (/(stream|running|load|progress|working)/.test(status)) return "working";
  if (/(done|complete|success|approv)/.test(status)) return "done";
  if (/(wait|queu|pend|retry)/.test(status)) return "waiting";
  if (block.kind === "user_query") return "working";
  return "idle";
}

/** Theme-driven badge tone for a dot state; interrupted reads muted, never alarming. */
export function agentDotBadgeTone(state: AgentDotState): StatusTone {
  switch (state) {
    case "working": return "accent";
    case "done": return "success";
    case "blocked":
    case "waiting": return "warning";
    case "failed": return "danger";
    case "interrupted":
    case "idle":
    default: return "neutral";
  }
}

/**
 * One interleaved history for command blocks and agent conversation.
 * Commands and agent entries keep distinct keys so ids never collide;
 * the live full-screen grid always trails as the newest terminal output.
 */
export function terminalItems(state: TerminalState, content: TerminalContent = "all"): TerminalItem[] {
  const showTerminal = content !== "agent";
  const showAgent = content !== "terminal";
  const items: TerminalItem[] = [];
  if (showTerminal) {
    for (const block of state.blocks) {
      items.push({ key: `command:${block.id}`, kind: "command", block, tsMs: block.tsMs });
    }
  }
  if (showAgent) {
    for (const block of state.agentBlocks) {
      items.push({ key: `agent:${block.id}`, kind: "agent", block, tsMs: block.tsMs });
    }
  }
  items.sort((a, b) => a.tsMs - b.tsMs);
  // A full-screen grid is terminal output, not agent conversation.
  if (showTerminal && state.altScreen) items.push({ key: "alt-screen", kind: "alt", screen: state.altScreen, tsMs: 0 });
  return items;
}

export function wireColorToCss(value: Readonly<WireColor> | null): string | undefined {
  return value ? `rgba(${value[0]}, ${value[1]}, ${value[2]}, ${value[3] / 255})` : undefined;
}

// These bit positions are part of the gateway's documented SGR wire contract.
const sgr = {
  bold: 1 << 0,
  dim: 1 << 1,
  italic: 1 << 2,
  underline: (1 << 3) | (1 << 4) | (1 << 5),
  inverse: 1 << 6,
  hidden: 1 << 7,
  strike: 1 << 8,
} as const;

export function spanPresentation(span: TerminalSpan, defaults: { foreground: string; background: string }): { text: string; style: TextStyle } {
  const foreground = wireColorToCss(span.fg) ?? defaults.foreground;
  const background = wireColorToCss(span.bg) ?? defaults.background;
  const inverse = (span.flags & sgr.inverse) !== 0;
  const underline = (span.flags & sgr.underline) !== 0;
  const strike = (span.flags & sgr.strike) !== 0;
  return {
    // Concealed output must stay concealed when copied or read by a screen reader.
    text: span.flags & sgr.hidden ? span.text.replace(/[^\n]/gu, " ") : span.text,
    style: {
      color: inverse ? background : foreground,
      backgroundColor: inverse ? foreground : background,
      fontWeight: span.flags & sgr.bold ? "700" : "400",
      fontStyle: span.flags & sgr.italic ? "italic" : "normal",
      opacity: span.flags & sgr.dim ? 0.65 : 1,
      textDecorationLine: underline && strike ? "underline line-through" : underline ? "underline" : strike ? "line-through" : "none",
    },
  };
}

/**
 * Small tolerance avoids disabling follow for subpixel differences at the end.
 * Non-finite or non-positive viewports fall back to following so short output
 * and malformed scroll events never strand the reader away from new output.
 */
export function isAtTerminalBottom(offset: number, viewportHeight: number, contentHeight: number): boolean {
  if (!Number.isFinite(offset) || !Number.isFinite(viewportHeight) || !Number.isFinite(contentHeight)) return true;
  if (viewportHeight <= 0 || contentHeight <= 0) return true;
  return contentHeight - viewportHeight - Math.max(0, offset) <= 2;
}
