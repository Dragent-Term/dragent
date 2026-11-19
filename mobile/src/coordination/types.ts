/**
 * Live agent-coordination domain (mobile owner).
 *
 * Wire contract (root owns the Go broker; the native worker owns
 * realtime/companion_coordination.rs when it lands — no such file exists
 * today, so this follows the brief verbatim and invents nothing else):
 * - `coordination.list` params: optional `project_id`; result
 *   `{ items: [{ id, title, project_path, status, task_count, done_count,
 *   running_count, blocked_count, needs_user, updated_at }], truncated }`.
 * - `coordination.get` params: `{ team_id }`; result
 *   `{ team: { id, title, project_path, status, goal, tasks: [{ id, title,
 *   status, agent, model, depends_on, result_summary }], messages: [{ id,
 *   from, label, kind, text, created_at }] }, truncated }`.
 * - Team status: planning/running/done/stopped. Task/message enums derive
 *   daemon `dragent/crates/dragent_host/src/team.rs` (`TaskStatus::key`,
 *   `MessageKind` snake_case). Readonly UI: no mutations cross the wire.
 */

/** Team lifecycle mirrors daemon `TeamStatus` (planning/running/done/stopped). */
export type CoordinationTeamStatus = "planning" | "running" | "done" | "stopped";

export const COORDINATION_TEAM_STATUSES: readonly CoordinationTeamStatus[] = Object.freeze([
  "planning",
  "running",
  "done",
  "stopped",
]);

export function isCoordinationTeamStatus(value: unknown): value is CoordinationTeamStatus {
  return (
    typeof value === "string" &&
    (COORDINATION_TEAM_STATUSES as readonly string[]).includes(value)
  );
}

/**
 * Task lifecycle mirrors daemon `TaskStatus::key` (eight states).
 * `needs_decision` is not an error: its review still asks for changes after
 * its rounds, and the lead or the person decides what happens next.
 */
export type CoordinationTaskStatus =
  | "pending"
  | "starting"
  | "running"
  | "blocked"
  | "done"
  | "failed"
  | "needs_decision"
  | "cancelled";

export const COORDINATION_TASK_STATUSES: readonly CoordinationTaskStatus[] = Object.freeze([
  "pending",
  "starting",
  "running",
  "blocked",
  "done",
  "failed",
  "needs_decision",
  "cancelled",
]);

export function isCoordinationTaskStatus(value: unknown): value is CoordinationTaskStatus {
  return (
    typeof value === "string" &&
    (COORDINATION_TASK_STATUSES as readonly string[]).includes(value)
  );
}

/** Message party mirrors daemon `Party`: stable kind, never a display name. */
export type CoordinationFrom = "lead" | "user" | "task" | "dragent";
/** Message kinds mirror daemon `MessageKind` snake_case. */
export type CoordinationMessageKind = "note" | "question" | "answer" | "handoff" | "finding" | "needs_user";

export const COORDINATION_MESSAGE_KINDS: readonly CoordinationMessageKind[] = Object.freeze([
  "note",
  "question",
  "answer",
  "handoff",
  "finding",
  "needs_user",
]);

export function isCoordinationMessageKind(value: unknown): value is CoordinationMessageKind {
  return (
    typeof value === "string" &&
    (COORDINATION_MESSAGE_KINDS as readonly string[]).includes(value)
  );
}

export interface CoordinationSummary {
  readonly id: string;
  readonly title: string;
  readonly project_path: string;
  readonly status: CoordinationTeamStatus;
  readonly task_count: number;
  readonly done_count: number;
  readonly running_count: number;
  readonly blocked_count: number;
  readonly needs_user: boolean;
  readonly updated_at: number;
}

export interface CoordinationListResult {
  readonly items: readonly CoordinationSummary[];
  readonly truncated: boolean;
}

export interface CoordinationTask {
  readonly id: string;
  readonly title: string;
  readonly status: CoordinationTaskStatus;
  readonly agent: string | null;
  readonly model: string | null;
  readonly depends_on: readonly string[];
  readonly result_summary: string | null;
  /** Why it's blocked or failed, when it is. */
  readonly reason: string | null;
  /** The phone's tab for the worker's terminal, when it's open. */
  readonly worker_tab_id: number | null;
  /** The person chose its model (it won't change on its own). */
  readonly model_locked: boolean;
  /** The model the desktop's model button shows; null: the agent's default. */
  readonly chosen_model: string | null;
  /** Unfinished work on a terminal agent: its model can change. */
  readonly can_set_model: boolean;
}

export interface CoordinationModelChoice {
  readonly id: string;
  readonly label: string;
}

/** What cleaning up a finished team does, by worktree ("t3 (folder)"). */
export interface CoordinationCleanup {
  /** Closed and removed; their branches stay. */
  readonly remove: readonly string[];
  /** Kept to look at the result. */
  readonly keep: readonly string[];
  /** Left alone: changes not committed. */
  readonly dirty: readonly string[];
}

export interface CoordinationMessage {
  readonly id: string;
  readonly from: CoordinationFrom;
  readonly label: string;
  readonly kind: CoordinationMessageKind;
  readonly text: string;
  readonly created_at: number;
}

export interface CoordinationTeam {
  readonly id: string;
  readonly title: string;
  readonly project_path: string;
  readonly status: CoordinationTeamStatus;
  readonly goal: string;
  readonly tasks: readonly CoordinationTask[];
  readonly messages: readonly CoordinationMessage[];
  /** Workers of one agent that run at once; null from older desktops. */
  readonly max_parallel: number | null;
  /** Whether ready tasks start on their own; null from older desktops. */
  readonly auto_start: boolean | null;
  /** Whether the lead answers its workers' permission prompts; null from older desktops. */
  readonly lead_answers_prompts: boolean | null;
  readonly cost: CoordinationCost | null;
  readonly lead_agent: string | null;
  /** The agents the team may use (`claude`, `codex`, …). */
  readonly roster: readonly string[];
  readonly outcome: string | null;
  /** What the team is waiting on you for, when anything. */
  readonly question: CoordinationQuestion | null;
  /** The phone's tab for the lead's terminal, when it's open. */
  readonly lead_tab_id: number | null;
  /** Models each unfinished task's agent may move to, by agent key. */
  readonly model_choices: Readonly<Record<string, readonly CoordinationModelChoice[]>>;
  /** At work, but its lead isn't running: offer a new lead. */
  readonly needs_new_lead: boolean;
  /** Finished, with worktrees that can go; null when nothing to clean up. */
  readonly cleanup: CoordinationCleanup | null;
}

export type CoordinationCost = "cheapest" | "balanced" | "best";

export const COORDINATION_COSTS: readonly { id: CoordinationCost; label: string; hint: string }[] = Object.freeze([
  { id: "cheapest", label: "Cheapest", hint: "The cheapest model that can do each task" },
  { id: "balanced", label: "Balanced", hint: "Strong models where they matter" },
  { id: "best", label: "Best", hint: "The strongest models, cost second" },
]);

export interface CoordinationQuestion {
  readonly id: string;
  readonly kind: CoordinationMessageKind;
  readonly label: string;
  readonly text: string;
}

export interface CoordinationDetailResult {
  readonly team: CoordinationTeam;
  readonly truncated: boolean;
}
