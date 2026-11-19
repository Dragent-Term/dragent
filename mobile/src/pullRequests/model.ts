/** A branch's pull request on GitHub and its checks, as the desktop reads them (`git.pr_status`). */

export type CheckState = "passed" | "failed" | "running" | "pending" | "skipped" | "cancelled" | "neutral";

export interface PullRequestCheck {
  readonly name: string;
  readonly workflow: string;
  readonly state: CheckState;
  readonly url: string | null;
}

export interface PullRequestStatus {
  readonly number: number;
  readonly title: string;
  readonly url: string | null;
  /** open | closed | merged */
  readonly state: string;
  readonly draft: boolean;
  /** approved | changes_requested | review_required | "" */
  readonly review: string;
  /** mergeable | conflicting | unknown */
  readonly mergeable: string;
  readonly head: string;
  readonly base: string;
  readonly checks: readonly PullRequestCheck[];
}

export type PullRequestReading = { readonly pr: PullRequestStatus } | { readonly pr: null; readonly reason: string };

const STATES: readonly CheckState[] = ["passed", "failed", "running", "pending", "skipped", "cancelled", "neutral"];
const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);
const str = (value: unknown, max: number) => (typeof value === "string" ? value.slice(0, max) : "");
/** Only a web link opens from the phone. */
const link = (value: unknown) => {
  const text = str(value, 2048);
  return /^https:\/\/[^\s]+$/.test(text) ? text : null;
};

export function readPullRequest(value: unknown): PullRequestReading | null {
  if (!isRecord(value)) return null;
  if (value.pr === null) return { pr: null, reason: str(value.reason, 300) || "This branch has no pull request yet." };
  const pr = value.pr;
  if (!isRecord(pr) || typeof pr.number !== "number") return null;
  const checks = Array.isArray(pr.checks)
    ? pr.checks.slice(0, 200).flatMap((entry) => {
        if (!isRecord(entry)) return [];
        const state = STATES.includes(entry.state as CheckState) ? (entry.state as CheckState) : "neutral";
        const name = str(entry.name, 200);
        return name ? [Object.freeze({ name, workflow: str(entry.workflow, 200), state, url: link(entry.url) })] : [];
      })
    : [];
  return {
    pr: Object.freeze({
      number: pr.number,
      title: str(pr.title, 500),
      url: link(pr.url),
      state: str(pr.state, 20),
      draft: pr.draft === true,
      review: str(pr.review, 40),
      mergeable: str(pr.mergeable, 20),
      head: str(pr.head, 255),
      base: str(pr.base, 255),
      checks: Object.freeze(checks),
    }),
  };
}

/** Failed first, then running and waiting, then the rest, as a CI page orders them. */
export const CHECK_ORDER: readonly CheckState[] = ["failed", "running", "pending", "passed", "cancelled", "skipped", "neutral"];

export function sortChecks(checks: readonly PullRequestCheck[]): PullRequestCheck[] {
  return [...checks].sort((a, b) => CHECK_ORDER.indexOf(a.state) - CHECK_ORDER.indexOf(b.state) || a.name.localeCompare(b.name));
}

export function countChecks(checks: readonly PullRequestCheck[]): Partial<Record<CheckState, number>> {
  const counts: Partial<Record<CheckState, number>> = {};
  for (const check of checks) counts[check.state] = (counts[check.state] ?? 0) + 1;
  return counts;
}

/** One line on the pull request's standing: what's in its way, if anything. */
export function readiness(pr: PullRequestStatus): { label: string; tone: "success" | "danger" | "warning" | "neutral" } {
  if (pr.state === "merged") return { label: "Merged", tone: "success" };
  if (pr.state === "closed") return { label: "Closed", tone: "neutral" };
  const counts = countChecks(pr.checks);
  if (pr.mergeable === "conflicting") return { label: "Conflicts with the base branch", tone: "danger" };
  if (counts.failed) return { label: `${counts.failed} check${counts.failed === 1 ? "" : "s"} failing`, tone: "danger" };
  if (counts.running || counts.pending) return { label: "Checks running", tone: "warning" };
  if (pr.review === "changes_requested") return { label: "Changes requested", tone: "warning" };
  if (pr.draft) return { label: "Draft", tone: "neutral" };
  if (pr.review === "review_required") return { label: "Waiting for review", tone: "neutral" };
  return { label: "Ready to merge", tone: "success" };
}
