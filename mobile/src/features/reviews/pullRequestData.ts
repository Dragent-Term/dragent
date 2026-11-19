import { REVIEW_SCOPES, REVIEW_STATES } from "../../preview/pullRequestSelectors";
import type { PreviewPullRequest, ReviewScope, ReviewState } from "../../preview/pullRequestTypes";
import type { PreviewSnapshot } from "../../preview/types";

/**
 * Reviews-owned UI formatting helpers only. All business logic —
 * selection, filtering, readiness, drafts — lives in the core
 * `src/preview/pullRequestSelectors.ts` and `PreviewStore`, which call
 * sites import directly.
 */

export function projectName(snapshot: PreviewSnapshot, projectId: string): string {
  return snapshot.projects.find((project) => project.id === projectId)?.name ?? "Unknown project";
}

/** Desktop `since()`: minutes, hours, then days from an epoch-millis timestamp. */
export function timeAgo(at: number, now: number = Date.now()): string {
  const minutes = Math.max(0, Math.floor((now - at) / 60000));
  if (!Number.isFinite(minutes)) return "unknown";
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes}m ago`;
  if (minutes < 1440) return `${Math.floor(minutes / 60)}h ago`;
  return `${Math.floor(minutes / 1440)}d ago`;
}

/** `#128` on GitHub, `!45` on GitLab — matches the desktop inbox reference. */
export function reviewReference(review: Pick<PreviewPullRequest, "provider" | "number">): string {
  return `${review.provider === "gitlab" ? "!" : "#"}${review.number}`;
}

export function scopeLabel(scope: ReviewScope): string {
  return REVIEW_SCOPES.find((entry) => entry.id === scope)?.label ?? "All reviews";
}

export function stateLabel(state: ReviewState): string {
  return REVIEW_STATES.find((entry) => entry.id === state)?.label ?? "Open";
}
