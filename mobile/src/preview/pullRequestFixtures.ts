import type { PreviewPullRequest } from "./pullRequestTypes";

/** Original provider-shaped examples. They are never fetched from real accounts. */
export function createPullRequestFixtures(): PreviewPullRequest[] {
  const time = Date.UTC(2026, 10, 1, 5);
  const navigation: PreviewPullRequest = {
    id: "lumen-navigation-pr", desktopId: "preview-mac", projectId: "lumen-web", workspaceId: "preview-navigation",
    provider: "github", number: 42, title: "Keep workspace navigation in context", author: "maya", state: "open",
    isDraft: false, createdByMe: false, reviewRequested: true,
    headBranch: "feat/workspace-navigation", baseBranch: "main", headOid: "a1b2c3d4e5f6", updatedAt: time,
    description: "Keep the project, workspace and branch visible while moving between open tabs.\n\nReview focus\n• Preserve the active workspace when returning from a tab.\n• Keep navigation usable on narrow windows.\n• Give keyboard users a predictable focus order.",
    reviewDecision: "required", conflicts: false, mergeRequirementsSatisfied: false, reviewers: ["You", "jo"], assignees: ["maya"], labels: ["navigation", "accessibility"],
    files: [{ path: "src/navigation.ts", status: "modified", added: 2, removed: 1, lines: [
      { kind: "context", text: "export function workspacePath(project, workspace) {" },
      { kind: "remove", text: "  return `/projects/${project}`;" },
      { kind: "add", text: "  return `/projects/${project}/workspaces/${workspace}`;" },
      { kind: "context", text: "}" },
      { kind: "add", text: "export const preserveWorkspace = true;" },
    ] }, { path: "src/navigation.test.ts", status: "added", added: 4, removed: 0, lines: [
      { kind: "add", text: 'test("keeps workspace identity", () => {' },
      { kind: "add", text: '  expect(workspacePath("web", "navigation"))' },
      { kind: "add", text: '    .toBe("/projects/web/workspaces/navigation");' },
      { kind: "add", text: "});" },
    ] }],
    commits: [
      { sha: "a1b2c3d", title: "Preserve workspace identity during navigation", author: "maya", at: time - 3_600_000 },
      { sha: "b2c3d4e", title: "Cover workspace paths with a focused check", author: "maya", at: time - 1_800_000 },
    ],
    checks: [
      { id: "types", name: "Type checking", status: "passed" },
      { id: "keyboard", name: "Keyboard navigation", status: "failed" },
      { id: "build", name: "Build", status: "passed" },
    ],
    activity: [
      { id: "opened", author: "maya", kind: "system", body: "Opened this pull request for review.", at: time - 1_500_000 },
      { id: "focus-note", author: "jo", kind: "review", body: "Please check focus when returning from a workspace tab. The branch context looks clear.", at: time - 900_000 },
      { id: "author-reply", author: "maya", kind: "comment", body: "Added a focused path check. I am investigating the remaining keyboard failure.", at: time - 300_000 },
    ],
  };
  return [navigation, {
    ...navigation, id: "lumen-empty-pr", number: 39, workspaceId: "preview-empty-states", title: "Clarify workspace empty states",
    author: "You", createdByMe: true, reviewRequested: false, headBranch: "fix/empty-states", headOid: "c3d4e5f6a7b8", updatedAt: time - 3_600_000,
    description: "Explain how to open or create the first workspace. Keep the primary action clear and the copy concise.",
    reviewDecision: "approved", mergeRequirementsSatisfied: true, reviewers: ["maya"], assignees: ["You"], labels: ["polish"],
    files: [{ path: "src/empty-state.ts", status: "modified", added: 1, removed: 1, lines: [
      { kind: "remove", text: 'export const title = "Nothing here";' },
      { kind: "add", text: 'export const title = "Create your first workspace";' },
    ] }], commits: [{ sha: "c3d4e5f", title: "Clarify the first workspace action", author: "You", at: time - 4_000_000 }],
    checks: [{ id: "build", name: "Build", status: "passed" }],
    activity: [{ id: "approved", author: "maya", kind: "review", body: "The first action is clear. Approved.", at: time - 3_700_000 }],
  }, {
    ...navigation, id: "notes-draft-mr", projectId: "field-notes", workspaceId: "preview-quick-start", provider: "gitlab", number: 18,
    title: "Draft: simplify the quick start", author: "You", isDraft: true, createdByMe: true, reviewRequested: false,
    headBranch: "docs/quick-start", headOid: "d4e5f6a7b8c9", updatedAt: time - 7_200_000,
    description: "A short guide for opening a project and choosing a workspace. Still gathering examples before requesting review.",
    reviewers: [], assignees: ["You"], labels: ["documentation"], reviewDecision: "required",
    files: [{ path: "docs/start.md", status: "modified", added: 1, removed: 0, lines: [
      { kind: "context", text: "# Quick start" }, { kind: "add", text: "Choose a project, then open a workspace." },
    ] }], commits: [{ sha: "d4e5f6a", title: "Outline the workspace quick start", author: "You", at: time - 8_000_000 }],
    checks: [{ id: "docs", name: "Documentation links", status: "running" }], activity: [],
  }, {
    ...navigation, id: "lumen-merged-pr", workspaceId: null, number: 34, title: "Reduce startup work", author: "You",
    state: "merged", createdByMe: true, reviewRequested: false, headBranch: "perf/startup", headOid: "e5f6a7b8c9d0", updatedAt: time - 86_400_000,
    description: "Move deferred work out of the initial render.", reviewDecision: "approved", files: [], checks: [],
    commits: [{ sha: "e5f6a7b", title: "Defer startup tasks until after the first render", author: "You", at: time - 90_000_000 }],
    reviewers: ["maya"], assignees: ["You"], labels: ["performance"],
    activity: [{ id: "merged", author: "maya", kind: "system", body: "Merged this pull request into main.", at: time - 86_400_000 }],
  }, {
    ...navigation, id: "notes-closed-mr", projectId: "field-notes", workspaceId: null, provider: "gitlab", number: 12,
    title: "Reorganize the documentation sidebar", author: "jo", state: "closed", reviewRequested: false,
    headBranch: "docs/sidebar", headOid: "f6a7b8c9d0e1", updatedAt: time - 172_800_000,
    description: "Closed while the documentation structure is being reconsidered.", files: [], commits: [], checks: [],
    activity: [{ id: "closed", author: "jo", kind: "system", body: "Closed this merge request.", at: time - 172_800_000 }],
  }, {
    ...navigation, id: "relay-retry-mr", desktopId: "preview-linux", projectId: "relay-api", workspaceId: "preview-retries",
    provider: "gitlab", number: 7, title: "Bound retry attempts", author: "sam", headBranch: "fix/retry-window", headOid: "a7b8c9d0e1f2",
    conflicts: true, reviewers: ["You"], assignees: ["sam"], labels: ["reliability"], description: "Bound retries and document the retry window. The target branch has changed.",
    files: [{ path: "src/retry-policy.ts", status: "modified", added: 1, removed: 1, lines: [
      { kind: "remove", text: "export const retryLimit = Infinity;" }, { kind: "add", text: "export const retryLimit = 3;" },
    ] }], commits: [{ sha: "a7b8c9d", title: "Bound retry attempts", author: "sam", at: time - 1_800_000 }],
    checks: [{ id: "tests", name: "API tests", status: "pending" }], activity: [],
  }];
}
