import type { WorkspaceChange } from "./types";

export type ReviewScope = "all" | "mine" | "requested" | "attention" | "ready";
export type ReviewState = "open" | "closed" | "merged" | "all";
export interface PreviewPullRequest {
  id: string;
  desktopId: string;
  projectId: string;
  workspaceId: string | null;
  provider: "github" | "gitlab";
  number: number;
  title: string;
  author: string;
  state: Exclude<ReviewState, "all">;
  isDraft: boolean;
  createdByMe: boolean;
  reviewRequested: boolean;
  headBranch: string;
  baseBranch: string;
  headOid: string;
  updatedAt: number;
  description: string;
  reviewDecision: "required" | "approved" | "changes-requested";
  conflicts: boolean;
  mergeRequirementsSatisfied: boolean;
  reviewers: readonly string[];
  assignees: readonly string[];
  labels: readonly string[];
  files: readonly (WorkspaceChange & { status: "added" | "modified" | "deleted" })[];
  commits: readonly { sha: string; title: string; author: string; at: number }[];
  checks: readonly { id: string; name: string; status: "passed" | "failed" | "running" | "pending" }[];
  activity: readonly { id: string; author: string; kind: "comment" | "review" | "system"; body: string; at: number }[];
}

/** Drafts belong to a provider review's exact head, independently of local Git changes. */
export interface PreviewPullRequestDraft {
  reviewId: string;
  headOid: string;
  viewedPaths: readonly string[];
  notes: readonly {
    id: string;
    path: string | null;
    /** Index in the exact head's unified diff; null for file/general drafts. */
    lineIndex: number | null;
    /** Optional range end for line-range drafts. */
    lineEnd?: number | null;
    body: string;
    kind: "comment" | "approve" | "request-changes";
  }[];
}

export interface NewPreviewPullRequest {
  projectId: string;
  workspaceId: string;
  provider: "github" | "gitlab";
  title: string;
  description: string;
  baseBranch: string;
}
