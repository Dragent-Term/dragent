import type { PreviewSnapshot } from "./types";
import type { PreviewPullRequest, PreviewPullRequestDraft, ReviewScope, ReviewState } from "./pullRequestTypes";

export const REVIEW_SCOPES = [
  { id: "all", label: "All reviews" }, { id: "mine", label: "Created by me" },
  { id: "requested", label: "Review requested" }, { id: "attention", label: "Needs attention" },
  { id: "ready", label: "Ready to merge" },
] as const;
export const REVIEW_STATES = [
  { id: "open", label: "Open" }, { id: "closed", label: "Closed" },
  { id: "merged", label: "Merged" }, { id: "all", label: "All states" },
] as const;

export function selectPullRequest(snapshot: PreviewSnapshot, desktopId: string, id: string): PreviewPullRequest | null {
  if (!snapshot.desktops.some((machine) => machine.id === desktopId)) return null;
  const review = snapshot.pullRequests.find((item) => item.id === id && item.desktopId === desktopId);
  if (!review || !snapshot.projects.some((project) => project.id === review.projectId && project.desktopId === desktopId)) return null;
  if (review.workspaceId !== null && !snapshot.workspaces.some((workspace) => workspace.id === review.workspaceId && workspace.projectId === review.projectId)) return null;
  return review;
}

export function pullRequestReadiness(review: PreviewPullRequest): {
  label: string; reason: string; tone: "neutral" | "success" | "warning" | "danger";
} {
  if (review.state === "merged") return { label: "Merged", reason: "This request has already been merged.", tone: "success" };
  if (review.state === "closed") return { label: "Closed", reason: "This request was closed without merging.", tone: "neutral" };
  if (review.isDraft) return { label: "Draft", reason: "The author is still preparing this request.", tone: "neutral" };
  if (review.conflicts) return { label: "Conflicts", reason: "Resolve conflicts with the target branch before merging.", tone: "danger" };
  if (review.checks.some((check) => check.status === "failed")) return { label: "Checks failing", reason: "A required check needs attention.", tone: "danger" };
  if (review.reviewDecision === "changes-requested") return { label: "Changes requested", reason: "Address the review feedback before merging.", tone: "danger" };
  if (review.checks.some((check) => check.status === "running" || check.status === "pending")) return { label: "Checks running", reason: "Wait for the checks to finish.", tone: "warning" };
  if (review.reviewDecision !== "approved") return { label: "Review needed", reason: "An approval is still required.", tone: "warning" };
  if (!review.mergeRequirementsSatisfied) return { label: "Check requirements", reason: "The provider has not confirmed all merge requirements.", tone: "warning" };
  return { label: "Ready to merge", reason: "The sample checks pass and the review is approved.", tone: "success" };
}

export function filterPullRequests(snapshot: PreviewSnapshot, desktopId: string, options: {
  projectId?: string | undefined; state?: ReviewState; scope?: ReviewScope; query?: string;
} = {}): readonly PreviewPullRequest[] {
  const { projectId, state = "open", scope = "all", query = "" } = options;
  if (!REVIEW_STATES.some((item) => item.id === state) || !REVIEW_SCOPES.some((item) => item.id === scope)) return [];
  const search = query.trim().toLocaleLowerCase();
  return snapshot.pullRequests.filter((review) => {
    if (!selectPullRequest(snapshot, desktopId, review.id)) return false;
    if (projectId && review.projectId !== projectId) return false;
    if (state !== "all" && review.state !== state) return false;
    if (scope === "mine" && !review.createdByMe) return false;
    if (scope === "requested" && !review.reviewRequested) return false;
    const readiness = pullRequestReadiness(review);
    if (scope === "ready" && readiness.label !== "Ready to merge") return false;
    if (scope === "attention" && !["Conflicts", "Checks failing", "Changes requested", "Review needed"].includes(readiness.label)) return false;
    const project = snapshot.projects.find((entry) => entry.id === review.projectId);
    return !search || [review.title, review.author, review.headBranch, review.baseBranch, review.provider,
      project?.name ?? "", `${review.provider === "gitlab" ? "!" : "#"}${review.number}`].join(" ").toLocaleLowerCase().includes(search);
  }).sort((a, b) => b.updatedAt - a.updatedAt || a.id.localeCompare(b.id));
}

export function selectPullRequestDraft(snapshot: PreviewSnapshot, review: PreviewPullRequest): PreviewPullRequestDraft {
  return snapshot.pullRequestDrafts.find((draft) => draft.reviewId === review.id && draft.headOid === review.headOid)
    ?? { reviewId: review.id, headOid: review.headOid, viewedPaths: [], notes: [] };
}
