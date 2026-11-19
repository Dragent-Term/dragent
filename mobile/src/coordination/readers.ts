/**
 * Strict readers for the live `coordination.list` / `coordination.get`
 * payloads, following the native host contract verbatim
 * (`dragent/app/src/realtime/companion_coordination.rs` as enforced by the
 * broker in `server/internal/relay/coordination_rpc.go`).
 *
 * Same style as `relayCatalog.ts` / `relayRpcMethods.ts`: unknown in, frozen
 * domain out or null; never throw, never coerce, never trim. Only snake_case
 * fields are read — no camelCase/`body`/`at` aliases. IDs are strings only
 * (native message IDs are projected as strings); numeric IDs and
 * trimmed/empty IDs are rejected. Timestamps must be safe integers.
 * Duplicate IDs and inconsistent counts fail closed.
 */
import {
  isCoordinationMessageKind,
  isCoordinationTaskStatus,
  isCoordinationTeamStatus,
  type CoordinationCleanup,
  type CoordinationCost,
  type CoordinationModelChoice,
  type CoordinationDetailResult,
  type CoordinationFrom,
  type CoordinationQuestion,
  type CoordinationListResult,
  type CoordinationMessage,
  type CoordinationSummary,
  type CoordinationTask,
  type CoordinationTeam,
} from "./types";

/** Broker-enforced wire limits (what the phone may receive). */
export const COORDINATION_LIMITS = Object.freeze({
  items: 100,
  tasks: 60,
  messages: 100,
  depends: 60,
  idChars: 128,
  titleChars: 2048,
  projectChars: 4096,
  goalChars: 32768,
  textChars: 32768,
  agentChars: 512,
  labelChars: 2048,
  fromChars: 512,
  kindChars: 64,
});

const COORDINATION_FROMS: readonly CoordinationFrom[] = Object.freeze([
  "lead",
  "user",
  "task",
  "dragent",
]);

function isCoordinationFrom(value: unknown): value is CoordinationFrom {
  return typeof value === "string" && (COORDINATION_FROMS as readonly string[]).includes(value);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** IDs: strings only, never empty, never trimmed, never coerced. */
function strictId(value: unknown): string | null {
  return typeof value === "string" && value.length >= 1 && value.length <= COORDINATION_LIMITS.idChars
    ? value
    : null;
}

/** Project paths: non-empty strings up to the native bound (never trimmed). */
function strictPath(value: unknown): string | null {
  return typeof value === "string" &&
      value.length >= 1 &&
      value.length <= COORDINATION_LIMITS.projectChars
    ? value
    : null;
}

/** Bounded text exactly as sent (empty allowed, matching the broker). */
function boundedText(value: unknown, limit: number): string | null {
  return typeof value === "string" && value.length <= limit ? value : null;
}

/** Present-or-null text: null/undefined stay null, otherwise bounded verbatim. */
function optionalText(value: unknown, limit: number): string | null {
  if (value === undefined || value === null) return null;
  return boundedText(value, limit);
}

/** Non-negative safe-integer count, exactly representable for the phone. */
function count(value: unknown): number | null {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : null;
}

/** Millisecond timestamps must stay exactly representable for the phone. */
function moment(value: unknown): number | null {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : null;
}

function boundedList(value: unknown, limit: number): unknown[] | null {
  return Array.isArray(value) && value.length <= limit ? value : null;
}

export function isUniqueIds(ids: readonly string[]): boolean {
  return new Set(ids).size === ids.length;
}

function parseSummary(entry: unknown): CoordinationSummary | null {
  if (!isRecord(entry)) return null;
  const id = strictId(entry.id);
  const title = boundedText(entry.title, COORDINATION_LIMITS.titleChars);
  const project_path = strictPath(entry.project_path);
  if (id === null || title === null || project_path === null) return null;
  if (!isCoordinationTeamStatus(entry.status)) return null;
  const task_count = count(entry.task_count);
  const done_count = count(entry.done_count);
  const running_count = count(entry.running_count);
  const blocked_count = count(entry.blocked_count);
  if (task_count === null || done_count === null || running_count === null || blocked_count === null) {
    return null;
  }
  if (done_count > task_count || running_count > task_count || blocked_count > task_count) return null;
  if (typeof entry.needs_user !== "boolean") return null;
  const updated_at = moment(entry.updated_at);
  if (updated_at === null) return null;
  return Object.freeze({
    id,
    title,
    project_path,
    status: entry.status,
    task_count,
    done_count,
    running_count,
    blocked_count,
    needs_user: entry.needs_user,
    updated_at,
  });
}

/** Validate a `coordination.list` result; null when not one. */
export function parseCoordinationListResult(raw: unknown): CoordinationListResult | null {
  if (!isRecord(raw)) return null;
  const list = boundedList(raw.items, COORDINATION_LIMITS.items);
  if (!list || typeof raw.truncated !== "boolean") return null;
  const items: CoordinationSummary[] = [];
  for (const entry of list) {
    const parsed = parseSummary(entry);
    if (!parsed) return null;
    items.push(parsed);
  }
  if (!isUniqueIds(items.map((item) => item.id))) return null;
  return Object.freeze({ items: Object.freeze(items), truncated: raw.truncated });
}

function tabId(raw: unknown): number | null {
  return typeof raw === "number" && Number.isSafeInteger(raw) && raw >= 0 ? raw : null;
}

const COSTS: readonly string[] = ["cheapest", "balanced", "best"];

function parseQuestion(raw: unknown): CoordinationQuestion | null {
  if (!isRecord(raw)) return null;
  const id = strictId(raw.id);
  const label = boundedText(raw.label, COORDINATION_LIMITS.labelChars);
  const text = boundedText(raw.text, COORDINATION_LIMITS.textChars);
  if (id === null || label === null || text === null || !isCoordinationMessageKind(raw.kind)) return null;
  return Object.freeze({ id, kind: raw.kind, label, text });
}

function names(raw: unknown): readonly string[] {
  return Object.freeze(
    Array.isArray(raw)
      ? raw.filter((name): name is string => typeof name === "string" && name.length <= COORDINATION_LIMITS.titleChars).slice(0, COORDINATION_LIMITS.tasks)
      : [],
  );
}

function parseCleanup(raw: unknown): CoordinationCleanup | null {
  if (!isRecord(raw)) return null;
  const remove = names(raw.remove);
  return remove.length === 0 ? null : Object.freeze({ remove, keep: names(raw.keep), dirty: names(raw.dirty) });
}

function parseModelChoices(raw: unknown): Readonly<Record<string, readonly CoordinationModelChoice[]>> {
  const out: Record<string, readonly CoordinationModelChoice[]> = {};
  if (!isRecord(raw)) return Object.freeze(out);
  for (const [agent, list] of Object.entries(raw).slice(0, 50)) {
    if (agent.length > 64 || !Array.isArray(list)) continue;
    const models: CoordinationModelChoice[] = [];
    for (const entry of list.slice(0, 200)) {
      if (!isRecord(entry)) continue;
      const id = optionalText(entry.id, 200);
      const label = optionalText(entry.label, 200);
      if (id && label) models.push(Object.freeze({ id, label }));
    }
    out[agent] = Object.freeze(models);
  }
  return Object.freeze(out);
}

function parseTask(entry: unknown): CoordinationTask | null {
  if (!isRecord(entry)) return null;
  const id = strictId(entry.id);
  const title = boundedText(entry.title, COORDINATION_LIMITS.titleChars);
  if (id === null || title === null) return null;
  if (!isCoordinationTaskStatus(entry.status)) return null;
  const agent = optionalText(entry.agent, COORDINATION_LIMITS.agentChars);
  if (entry.agent !== undefined && entry.agent !== null && agent === null) return null;
  const model = optionalText(entry.model, COORDINATION_LIMITS.agentChars);
  if (entry.model !== undefined && entry.model !== null && model === null) return null;
  const dependsRaw = boundedList(entry.depends_on, COORDINATION_LIMITS.depends);
  if (!dependsRaw) return null;
  const depends_on: string[] = [];
  for (const dep of dependsRaw) {
    const depId = strictId(dep);
    if (depId === null) return null;
    depends_on.push(depId);
  }
  const summaryRaw = entry.result_summary;
  let result_summary: string | null = null;
  if (summaryRaw !== undefined && summaryRaw !== null) {
    result_summary = boundedText(summaryRaw, COORDINATION_LIMITS.goalChars);
    if (result_summary === null) return null;
  }
  const reason = typeof entry.reason === "string" ? entry.reason.slice(0, COORDINATION_LIMITS.goalChars) : null;
  return Object.freeze({
    id,
    title,
    status: entry.status,
    agent,
    model,
    depends_on: Object.freeze(depends_on),
    result_summary,
    reason,
    worker_tab_id: tabId(entry.worker_tab_id),
    model_locked: entry.model_locked === true,
    chosen_model: optionalText(entry.chosen_model, COORDINATION_LIMITS.agentChars),
    can_set_model: entry.can_set_model === true,
  });
}

function parseMessage(entry: unknown): CoordinationMessage | null {
  if (!isRecord(entry)) return null;
  // String IDs only: the broker normalizes native message IDs to strings.
  const id = strictId(entry.id);
  const from = isCoordinationFrom(entry.from) ? entry.from : null;
  const label = boundedText(entry.label, COORDINATION_LIMITS.labelChars);
  if (id === null || from === null || label === null) return null;
  if (!isCoordinationMessageKind(entry.kind)) return null;
  const text = boundedText(entry.text, COORDINATION_LIMITS.textChars);
  if (text === null) return null;
  const created_at = moment(entry.created_at);
  if (created_at === null) return null;
  return Object.freeze({ id, from, label, kind: entry.kind, text, created_at });
}

function parseTeam(raw: unknown): CoordinationTeam | null {
  if (!isRecord(raw)) return null;
  const id = strictId(raw.id);
  const title = boundedText(raw.title, COORDINATION_LIMITS.titleChars);
  const project_path = strictPath(raw.project_path);
  if (id === null || title === null || project_path === null) return null;
  if (!isCoordinationTeamStatus(raw.status)) return null;
  const goal = boundedText(raw.goal, COORDINATION_LIMITS.goalChars);
  if (goal === null) return null;
  const tasksRaw = boundedList(raw.tasks, COORDINATION_LIMITS.tasks);
  const messagesRaw = boundedList(raw.messages, COORDINATION_LIMITS.messages);
  if (!tasksRaw || !messagesRaw) return null;
  const tasks: CoordinationTask[] = [];
  for (const entry of tasksRaw) {
    const parsed = parseTask(entry);
    if (!parsed) return null;
    tasks.push(parsed);
  }
  if (!isUniqueIds(tasks.map((task) => task.id))) return null;
  const messages: CoordinationMessage[] = [];
  for (const entry of messagesRaw) {
    const parsed = parseMessage(entry);
    if (!parsed) return null;
    messages.push(parsed);
  }
  if (!isUniqueIds(messages.map((message) => message.id))) return null;
  const max_parallel =
    typeof raw.max_parallel === "number" && Number.isInteger(raw.max_parallel) && raw.max_parallel >= 1 && raw.max_parallel <= 64
      ? raw.max_parallel
      : null;
  const auto_start = typeof raw.auto_start === "boolean" ? raw.auto_start : null;
  return Object.freeze({
    id,
    title,
    project_path,
    status: raw.status,
    goal,
    tasks: Object.freeze(tasks),
    messages: Object.freeze(messages),
    max_parallel,
    auto_start,
    lead_answers_prompts: typeof raw.lead_answers_prompts === "boolean" ? raw.lead_answers_prompts : null,
    cost: typeof raw.cost === "string" && COSTS.includes(raw.cost) ? (raw.cost as CoordinationCost) : null,
    lead_agent: optionalText(raw.lead_agent, COORDINATION_LIMITS.agentChars),
    roster: Object.freeze(
      Array.isArray(raw.roster)
        ? raw.roster.filter((agent): agent is string => typeof agent === "string" && agent.length <= 64).slice(0, 50)
        : [],
    ),
    outcome: typeof raw.outcome === "string" ? raw.outcome.slice(0, COORDINATION_LIMITS.textChars) : null,
    question: parseQuestion(raw.question),
    lead_tab_id: tabId(raw.lead_tab_id),
    model_choices: parseModelChoices(raw.model_choices),
    needs_new_lead: raw.needs_new_lead === true,
    cleanup: parseCleanup(raw.cleanup),
  });
}

/** Validate a `coordination.get` result; null when not one. */
export function parseCoordinationDetailResult(raw: unknown): CoordinationDetailResult | null {
  if (!isRecord(raw)) return null;
  if (typeof raw.truncated !== "boolean") return null;
  const team = parseTeam(raw.team);
  if (!team) return null;
  return Object.freeze({ team, truncated: raw.truncated });
}
