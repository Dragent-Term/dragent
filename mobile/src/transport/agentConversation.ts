/**
 * The built-in agent's conversation as the desktop streams it (`agent.<tab>`).
 *
 * The desktop sends the whole row order with every message, but only the
 * rows that changed since the last one. This module validates each message
 * and folds it into an ordered, bounded list of rows. Pure data: no React, no
 * transport, no theme. Everything here arrives inside the encrypted channel.
 */

export const AGENT_CONVERSATION_LIMITS = Object.freeze({
  rows: 500,
  textChars: 20_000,
  options: 24,
  optionChars: 400,
  diffFiles: 24,
  diffLines: 240,
  todoItems: 100,
});

export interface AgentDiffLine {
  /** `add`, `del` or `ctx`. */
  readonly kind: "add" | "del" | "ctx";
  readonly text: string;
}

export interface AgentDiffFile {
  readonly path: string;
  readonly added: number;
  readonly removed: number;
  readonly lines: readonly AgentDiffLine[];
}

interface RowBase {
  readonly id: number;
  readonly tsMs: number;
  /** The desktop cut long content from this row. */
  readonly clipped: boolean;
}

export type AgentRow =
  | (RowBase & { readonly kind: "user_query"; readonly text: string; readonly author: string | null })
  | (RowBase & { readonly kind: "agent_message"; readonly text: string; readonly streaming: boolean })
  | (RowBase & { readonly kind: "system_note"; readonly text: string })
  | (RowBase & { readonly kind: "thinking"; readonly text: string; readonly complete: boolean })
  | (RowBase & {
      readonly kind: "tool_step";
      readonly name: string;
      readonly args: string;
      readonly status: "running" | "done" | "error";
      readonly result: string | null;
      readonly durationMs: number | null;
      readonly diff: readonly AgentDiffFile[];
    })
  | (RowBase & {
      readonly kind: "action_result";
      readonly command: string;
      readonly exitCode: number | null;
      readonly output: string;
    })
  | (RowBase & {
      readonly kind: "approval";
      readonly ref: string | null;
      readonly command: string;
      readonly reason: string;
      readonly status: "pending" | "approved" | "approved_always" | "rejected";
    })
  | (RowBase & {
      readonly kind: "question";
      readonly ref: string | null;
      readonly header: string;
      readonly question: string;
      readonly options: readonly { readonly label: string; readonly description: string }[];
      readonly picked: readonly string[];
      readonly multi: boolean;
      readonly custom: boolean;
      /** 0-based position among the questions asked together. */
      readonly index: number;
      readonly count: number;
      readonly status: "pending" | "answered" | "dismissed";
    })
  | (RowBase & {
      readonly kind: "plan_review";
      readonly ref: string | null;
      readonly content: string;
      readonly status: "pending" | "approved" | "refining";
    })
  | (RowBase & { readonly kind: "queued"; readonly ref: string | null; readonly text: string })
  | (RowBase & { readonly kind: "loading"; readonly message: string })
  | (RowBase & { readonly kind: "error"; readonly message: string })
  | (RowBase & { readonly kind: "turn_end"; readonly durationMs: number | null; readonly note: string | null })
  | (RowBase & {
      readonly kind: "todos";
      readonly items: readonly { readonly content: string; readonly status: string }[];
    })
  /** A row kind this build doesn't know: shown as a plain line, never dropped silently. */
  | (RowBase & { readonly kind: "unknown"; readonly label: string; readonly text: string });

export interface AgentConversationState {
  /** False until the desktop pane has sent its conversation. */
  readonly ready: boolean;
  /** A turn is running: Stop applies and a new prompt queues. */
  readonly busy: boolean;
  /** Older rows were left out by the desktop. */
  readonly clipped: boolean;
  readonly rows: readonly AgentRow[];
}

export const EMPTY_AGENT_CONVERSATION: AgentConversationState = Object.freeze({
  ready: false,
  busy: false,
  clipped: false,
  rows: Object.freeze([]),
});

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function text(value: unknown, limit: number = AGENT_CONVERSATION_LIMITS.textChars): string {
  if (typeof value !== "string") return "";
  return value.length > limit ? `${value.slice(0, limit)}…` : value;
}

function optionalText(value: unknown): string | null {
  return typeof value === "string" && value !== "" ? text(value) : null;
}

function safeInt(value: unknown): number | null {
  return typeof value === "number" && Number.isSafeInteger(value) ? value : null;
}

function oneOf<T extends string>(value: unknown, allowed: readonly T[], fallback: T): T {
  return typeof value === "string" && (allowed as readonly string[]).includes(value) ? (value as T) : fallback;
}

function reference(value: unknown): string | null {
  return typeof value === "string" && /^[0-9a-fA-F-]{36}$/u.test(value) ? value : null;
}

function parseDiff(value: unknown): readonly AgentDiffFile[] {
  if (!isRecord(value) || !Array.isArray(value.files)) return [];
  const files: AgentDiffFile[] = [];
  for (const file of value.files.slice(0, AGENT_CONVERSATION_LIMITS.diffFiles)) {
    if (!isRecord(file)) continue;
    const lines: AgentDiffLine[] = [];
    let added = 0;
    let removed = 0;
    const raw = Array.isArray(file.lines) ? file.lines : [];
    for (const line of raw.slice(0, AGENT_CONVERSATION_LIMITS.diffLines)) {
      if (!isRecord(line)) continue;
      const tag = typeof line.kind === "string" ? line.kind.toLowerCase() : "";
      const kind = tag.startsWith("add") || tag === "+" ? "add" : tag.startsWith("del") || tag.startsWith("rem") || tag === "-" ? "del" : "ctx";
      if (kind === "add") added += 1;
      if (kind === "del") removed += 1;
      lines.push({ kind, text: text(line.text ?? line.content, 2_000) });
    }
    files.push({
      path: text(file.path, 1_000),
      added: safeInt(file.added) ?? added,
      removed: safeInt(file.removed) ?? removed,
      lines,
    });
  }
  return files;
}

/** One row from the wire, or null when it has no usable identity. */
export function parseAgentRow(raw: unknown): AgentRow | null {
  if (!isRecord(raw)) return null;
  const id = safeInt(raw.id);
  if (id === null || id < 0) return null;
  const base: RowBase = { id, tsMs: safeInt(raw.ts_ms) ?? 0, clipped: raw.clipped === true };
  switch (raw.kind) {
    case "user_query":
      return { ...base, kind: "user_query", text: text(raw.text), author: isRecord(raw.author) ? optionalText(raw.author.display_name) : null };
    case "agent_message":
      return { ...base, kind: "agent_message", text: text(raw.text), streaming: raw.streaming === true };
    case "system_note":
      return { ...base, kind: "system_note", text: text(raw.text) };
    case "thinking":
      return { ...base, kind: "thinking", text: text(raw.content), complete: raw.complete === true };
    case "tool_step":
      return {
        ...base,
        kind: "tool_step",
        name: text(raw.name, 200),
        args: text(raw.args, 2_000),
        status: oneOf(raw.status, ["running", "done", "error"] as const, "done"),
        result: optionalText(raw.result),
        durationMs: safeInt(raw.duration_ms),
        diff: parseDiff(raw.diff),
      };
    case "action_result":
      return { ...base, kind: "action_result", command: text(raw.command, 4_000), exitCode: safeInt(raw.exit_code), output: text(raw.output_preview) };
    case "approval":
      return {
        ...base,
        kind: "approval",
        ref: reference(raw.ref),
        command: text(raw.command, 8_000),
        reason: text(raw.reason, 4_000),
        status: oneOf(raw.status, ["pending", "approved", "approved_always", "rejected"] as const, "pending"),
      };
    case "question": {
      const labels = Array.isArray(raw.options) ? raw.options.slice(0, AGENT_CONVERSATION_LIMITS.options) : [];
      const descriptions = Array.isArray(raw.descriptions) ? raw.descriptions : [];
      const options = labels
        .map((label, index) => ({
          label: text(label, AGENT_CONVERSATION_LIMITS.optionChars),
          description: text(descriptions[index], AGENT_CONVERSATION_LIMITS.optionChars),
        }))
        .filter((option) => option.label !== "");
      const picked = Array.isArray(raw.picked)
        ? raw.picked.filter((value): value is string => typeof value === "string").slice(0, AGENT_CONVERSATION_LIMITS.options)
        : [];
      return {
        ...base,
        kind: "question",
        ref: reference(raw.ref),
        header: text(raw.header, 200),
        question: text(raw.question, 4_000),
        options,
        picked,
        multi: raw.multi === true,
        custom: raw.custom !== false,
        index: Math.max(0, safeInt(raw.index) ?? 0),
        count: Math.max(1, safeInt(raw.count) ?? 1),
        status: oneOf(raw.status, ["pending", "answered", "dismissed"] as const, "pending"),
      };
    }
    case "plan_review":
      return {
        ...base,
        kind: "plan_review",
        ref: reference(raw.ref),
        content: text(raw.content),
        status: oneOf(raw.status, ["pending", "approved", "refining"] as const, "pending"),
      };
    case "queued":
      return { ...base, kind: "queued", ref: reference(raw.ref), text: text(raw.text) };
    case "loading":
      return { ...base, kind: "loading", message: text(raw.message, 400) };
    case "error":
      return { ...base, kind: "error", message: text(raw.message, 4_000) };
    case "turn_end":
      return { ...base, kind: "turn_end", durationMs: safeInt(raw.duration_ms), note: optionalText(raw.status_note) };
    case "todos": {
      const items = Array.isArray(raw.items) ? raw.items.slice(0, AGENT_CONVERSATION_LIMITS.todoItems) : [];
      return {
        ...base,
        kind: "todos",
        items: items.filter(isRecord).map((item) => ({ content: text(item.content, 1_000), status: text(item.status, 40) })),
      };
    }
    default:
      return {
        ...base,
        kind: "unknown",
        label: text(raw.kind, 60) || "Update",
        text: text(raw.text ?? raw.message ?? raw.content),
      };
  }
}

export type AgentUpdateResult =
  | { readonly ok: true; readonly state: AgentConversationState }
  /** The message named a row this phone was never sent: ask for everything again. */
  | { readonly ok: false; readonly reason: "resync" | "unreadable" };

/**
 * Fold one stream message into the conversation. `snapshot` is the first
 * message of a subscription: it replaces everything held so far.
 */
export function applyAgentUpdate(
  previous: AgentConversationState,
  payload: unknown,
  snapshot: boolean,
): AgentUpdateResult {
  if (!isRecord(payload) || !Array.isArray(payload.order) || !Array.isArray(payload.blocks)) {
    return { ok: false, reason: "unreadable" };
  }
  if (payload.order.length > AGENT_CONVERSATION_LIMITS.rows || payload.blocks.length > AGENT_CONVERSATION_LIMITS.rows) {
    return { ok: false, reason: "unreadable" };
  }
  const held = new Map<number, AgentRow>();
  if (!snapshot) for (const row of previous.rows) held.set(row.id, row);
  for (const raw of payload.blocks) {
    const row = parseAgentRow(raw);
    if (row === null) return { ok: false, reason: "unreadable" };
    held.set(row.id, row);
  }
  const rows: AgentRow[] = [];
  const seen = new Set<number>();
  for (const rawId of payload.order) {
    const id = safeInt(rawId);
    if (id === null || seen.has(id)) return { ok: false, reason: "unreadable" };
    seen.add(id);
    const row = held.get(id);
    if (!row) return { ok: false, reason: "resync" };
    rows.push(row);
  }
  return {
    ok: true,
    state: Object.freeze({
      ready: payload.ready !== false,
      busy: payload.busy === true,
      clipped: payload.clipped === true,
      rows: Object.freeze(rows),
    }),
  };
}

/** The card the person must answer before the turn can go on, if any. */
export function pendingAgentRequest(state: AgentConversationState): AgentRow | null {
  for (let index = state.rows.length - 1; index >= 0; index -= 1) {
    const row = state.rows[index]!;
    if ((row.kind === "approval" || row.kind === "question" || row.kind === "plan_review") && row.status === "pending") {
      return row;
    }
  }
  return null;
}

/** "1m 20s", "8s"; null for an unknown length. */
export function agentDurationLabel(durationMs: number | null): string | null {
  if (durationMs === null || durationMs < 0) return null;
  const seconds = Math.round(durationMs / 1000);
  if (seconds < 60) return `${Math.max(seconds, 1)}s`;
  const minutes = Math.floor(seconds / 60);
  const rest = seconds % 60;
  if (minutes < 60) return rest === 0 ? `${minutes}m` : `${minutes}m ${rest}s`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h ${minutes % 60}m`;
}
