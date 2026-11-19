import type { TerminalSurfaceState, WireAgentBlock, WireColor } from "../transport";
import { TERMINAL_LIMITS as limits } from "./limits";
import type { TerminalAgentBlock, TerminalLine, TerminalSpan } from "./types";

export function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function unsigned(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

export function integer(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value);
}

export function text(value: unknown, limit: number = limits.metadataCharacters): string {
  return typeof value === "string" ? value.slice(0, limit) : "";
}

export function optionalText(value: unknown): string | null {
  return typeof value === "string" ? text(value) : null;
}

function color(value: unknown): WireColor | null {
  if (!Array.isArray(value) || value.length !== 4 || !value.every((n) => unsigned(n) && n <= 255)) return null;
  return [value[0], value[1], value[2], value[3]] as WireColor;
}

export function normalizeLine(value: unknown, index: number): TerminalLine | null {
  if (!record(value) || !Array.isArray(value.spans)) return null;
  const spans: TerminalSpan[] = [];
  let remaining: number = limits.lineCharacters;
  let truncated = value.spans.length > limits.spansPerLine;
  for (const candidate of value.spans.slice(0, limits.spansPerLine)) {
    if (!record(candidate) || typeof candidate.text !== "string") continue;
    const content = candidate.text.slice(0, remaining);
    truncated ||= candidate.text.length > remaining;
    remaining -= content.length;
    if (content.length > 0) {
      spans.push({
        text: content,
        fg: color(candidate.fg),
        bg: color(candidate.bg),
        flags: unsigned(candidate.flags) ? candidate.flags & 0x7ff : 0,
      });
    }
  }
  return { index, spans, truncated };
}

export function normalizeSurface(value: unknown): TerminalSurfaceState | null {
  if (!record(value)) return null;
  const command = text(value.command_input);
  const ai = text(value.ai_input);
  return {
    command_input: command,
    command_ghost: optionalText(value.command_ghost),
    command_cursor: unsigned(value.command_cursor) ? Math.min(value.command_cursor, command.length) : 0,
    ai_tool: optionalText(value.ai_tool),
    ai_input: ai,
    ai_ghost: optionalText(value.ai_ghost),
    ai_cursor: unsigned(value.ai_cursor) ? Math.min(value.ai_cursor, ai.length) : 0,
    ai_status: optionalText(value.ai_status),
    attachments: Array.isArray(value.attachments)
      ? value.attachments.slice(0, 16).filter((entry): entry is string => typeof entry === "string").map((entry) => text(entry))
      : [],
    activity: optionalText(value.activity),
    side_command: optionalText(value.side_command),
    command_running: value.command_running === true,
    cwd: optionalText(value.cwd),
  };
}

const agentLabels: Record<WireAgentBlock["kind"], string> = {
  user_query: "User",
  agent_message: "Agent",
  system_note: "System note",
  thinking: "Thinking",
  tool_step: "Tool",
  action_result: "Command result",
  approval: "Approval request · read only",
  question: "Question · read only",
  plan_review: "Plan review · read only",
  queued: "Queued",
  loading: "Agent activity",
  error: "Agent error",
  turn_group: "Agent turn",
  activity_group: "Agent activity",
  empty: "Agent update",
};

interface AgentBudget { characters: number; nodes: number; truncated: boolean }

/** Only known display fields are read. Links, tool calls and approvals stay inert. */
function agentText(value: unknown, budget: AgentBudget, depth: number): string {
  if (!record(value)) return "";
  if (depth > limits.agentDepth || budget.nodes-- <= 0) {
    budget.truncated = true;
    return "";
  }
  const take = (entry: unknown): string => {
    if (typeof entry !== "string") return "";
    const result = entry.slice(0, budget.characters);
    budget.truncated ||= entry.length > result.length;
    budget.characters -= result.length;
    return result;
  };
  let fields: unknown[];
  switch (value.kind) {
    case "turn_group":
    case "activity_group": {
      const children = value.kind === "turn_group" ? value.children : value.blocks;
      if (!Array.isArray(children)) return "";
      budget.truncated ||= children.length > limits.agentNodes;
      return children.slice(0, limits.agentNodes)
        .map((child) => agentText(child, budget, depth + 1)).filter(Boolean).join("\n");
    }
    case "thinking":
    case "plan_review": fields = [value.content]; break;
    case "tool_step": fields = [value.name, value.args, value.result]; break;
    case "action_result": fields = [value.command, value.output_preview]; break;
    case "approval": fields = [value.command, value.reason]; break;
    case "question":
      fields = [value.header, value.question, ...(Array.isArray(value.options) ? value.options.slice(0, 32) : [])];
      break;
    case "loading":
    case "error": fields = [value.message]; break;
    case "empty": return "";
    default: fields = [value.text];
  }
  return fields.map(take).filter(Boolean).join("\n");
}

export function normalizeAgents(values: readonly unknown[]): { blocks: TerminalAgentBlock[]; truncated: boolean } {
  const blocks = new Map<number, TerminalAgentBlock>();
  let truncated = values.length > limits.agentBlocks;
  for (const value of values.slice(-limits.agentBlocks)) {
    if (!record(value) || !unsigned(value.id) || typeof value.kind !== "string") continue;
    const kind = Object.prototype.hasOwnProperty.call(agentLabels, value.kind)
      ? value.kind as WireAgentBlock["kind"] : "unknown";
    if (kind === "empty") continue;
    const budget: AgentBudget = { characters: limits.agentCharacters, nodes: limits.agentNodes, truncated: false };
    const content = agentText(value, budget, 0);
    truncated ||= budget.truncated;
    const author = record(value.author) ? text(value.author.display_name, 128) : "";
    const label = kind === "unknown" ? "Agent update" : agentLabels[kind];
    blocks.set(value.id, {
      id: value.id,
      tsMs: integer(value.ts_ms) ? value.ts_ms : 0,
      kind,
      label: kind === "user_query" && author ? `${label} · ${author}` : label,
      // Separators also count toward the final storage bound.
      text: content.slice(0, limits.agentCharacters),
      status: optionalText(value.status) ?? (value.streaming === true ? "Streaming" : null),
      truncated: budget.truncated || content.length > limits.agentCharacters,
    });
  }
  return { blocks: [...blocks.values()], truncated };
}
