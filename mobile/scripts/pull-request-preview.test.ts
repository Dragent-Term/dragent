import assert from "node:assert/strict";
import { test } from "node:test";
import { PreviewStore, PREVIEW_LIMITS } from "../src/preview/PreviewStore";
import { filterPullRequests, pullRequestReadiness, selectPullRequest, selectPullRequestDraft } from "../src/preview/pullRequestSelectors";
import type { NewPreviewPullRequest, PreviewPullRequest } from "../src/preview/pullRequestTypes";

test("review inbox filters and references remain within the selected machine and project", () => {
  const snapshot = new PreviewStore().getSnapshot();
  assert.equal(filterPullRequests(snapshot, "preview-mac").length, 3);
  assert.equal(filterPullRequests(snapshot, "preview-linux").length, 1);
  assert.equal(filterPullRequests(snapshot, "missing", { state: "all" }).length, 0);
  assert.equal(filterPullRequests(snapshot, "preview-mac", { projectId: "relay-api", state: "all" }).length, 0);
  assert.equal(filterPullRequests(snapshot, "preview-mac", { scope: "mine" }).length, 2);
  assert.equal(filterPullRequests(snapshot, "preview-mac", { scope: "requested" }).length, 1);
  assert.equal(filterPullRequests(snapshot, "preview-mac", { scope: "ready" })[0]?.id, "lumen-empty-pr");
  assert.equal(filterPullRequests(snapshot, "preview-mac", { scope: "attention" })[0]?.id, "lumen-navigation-pr");
  assert.equal(filterPullRequests(snapshot, "preview-mac", { state: "merged" })[0]?.id, "lumen-merged-pr");
  assert.equal(filterPullRequests(snapshot, "preview-mac", { query: "!18" })[0]?.provider, "gitlab");
  assert.equal(filterPullRequests(snapshot, "preview-mac", { query: "  MAYA  " })[0]?.id, "lumen-navigation-pr");
  assert.equal(filterPullRequests(snapshot, "preview-linux", { query: "#42" }).length, 0);
  for (const review of snapshot.pullRequests) assert.ok(selectPullRequest(snapshot, review.desktopId, review.id));
  const invalid = { ...snapshot, pullRequests: snapshot.pullRequests.map((review) => ({ ...review, workspaceId: "preview-retries" })) };
  assert.equal(selectPullRequest(invalid, "preview-mac", "lumen-navigation-pr"), null);
});

test("readiness respects state, draft, blockers, checks, approval and confirmed merge requirements", () => {
  const review = new PreviewStore().getSnapshot().pullRequests[0]!;
  const status = (changes: Partial<PreviewPullRequest>) => pullRequestReadiness({ ...review, ...changes }).label;
  assert.equal(status({}), "Checks failing");
  assert.equal(status({ state: "merged", isDraft: true, conflicts: true }), "Merged");
  assert.equal(status({ state: "closed" }), "Closed");
  assert.equal(status({ isDraft: true, conflicts: true }), "Draft");
  assert.equal(status({ conflicts: true }), "Conflicts");
  assert.equal(status({ checks: [], reviewDecision: "changes-requested" }), "Changes requested");
  assert.equal(status({ checks: [{ id: "ci", name: "CI", status: "running" }], reviewDecision: "approved" }), "Checks running");
  assert.equal(status({ checks: [], reviewDecision: "required" }), "Review needed");
  assert.equal(status({ checks: [], reviewDecision: "approved", mergeRequirementsSatisfied: false }), "Check requirements");
  assert.equal(status({ checks: [], reviewDecision: "approved", mergeRequirementsSatisfied: true }), "Ready to merge");
});

test("provider review file progress and drafts reject stale heads and cross-machine writes", () => {
  const store = new PreviewStore();
  const snapshot = store.getSnapshot();
  const review = snapshot.pullRequests[0]!;
  const file = review.files[0]!.path;
  assert.equal(store.markPullRequestFileViewed("preview-linux", review.id, review.headOid, file, true), false);
  assert.equal(store.markPullRequestFileViewed("preview-mac", review.id, "stale", file, true), false);
  assert.equal(store.markPullRequestFileViewed("preview-mac", review.id, review.headOid, "missing", true), false);
  assert.equal(store.addPullRequestNote("preview-mac", review.id, "stale", { path: file, body: "Note", kind: "comment" }), false);
  assert.equal(store.getSnapshot(), snapshot);
  assert.ok(store.markPullRequestFileViewed("preview-mac", review.id, review.headOid, file, true));
  assert.ok(store.markPullRequestFileViewed("preview-mac", review.id, review.headOid, file, true));
  assert.ok(store.addPullRequestNote("preview-mac", review.id, review.headOid, { path: file, body: "Preserve workspace context.", kind: "request-changes" }));
  let draft = selectPullRequestDraft(store.getSnapshot(), review);
  assert.deepEqual(draft.viewedPaths, [file]);
  assert.equal(draft.notes[0]?.kind, "request-changes");
  assert.equal(store.getSnapshot().pullRequests, snapshot.pullRequests, "Saving a draft cannot change provider review state");
  assert.equal(store.getSnapshot().reviews.length, 0, "Provider drafts cannot mark local workspace changes reviewed");
  assert.deepEqual(selectPullRequestDraft(store.getSnapshot(), { ...review, headOid: "new-head" }).notes, []);
  assert.ok(store.markPullRequestFileViewed("preview-mac", review.id, review.headOid, file, false));
  draft = selectPullRequestDraft(store.getSnapshot(), review);
  assert.deepEqual(draft.viewedPaths, []);
  assert.ok(Object.isFrozen(draft.notes));
});

test("line drafts preserve exact-head anchors and reject invalid or general-note anchors", () => {
  const store = new PreviewStore();
  const review = store.getSnapshot().pullRequests[0]!;
  const file = review.files[0]!;
  const note = { path: file.path, lineIndex: 0, body: "Keep this route scoped to its workspace.", kind: "comment" as const };
  for (const lineIndex of [-1, 0.5, NaN, file.lines.length]) {
    assert.equal(store.addPullRequestNote("preview-mac", review.id, review.headOid, { ...note, lineIndex }), false);
  }
  assert.equal(store.addPullRequestNote("preview-mac", review.id, review.headOid, { ...note, path: null }), false);
  assert.equal(store.addPullRequestNote("preview-mac", review.id, "older-head", note), false);
  assert.deepEqual(store.getSnapshot().pullRequestDrafts, []);
  assert.ok(store.addPullRequestNote("preview-mac", review.id, review.headOid, note));
  assert.equal(selectPullRequestDraft(store.getSnapshot(), review).notes[0]?.lineIndex, 0);
  assert.ok(store.addPullRequestNote("preview-mac", review.id, review.headOid, { ...note, lineIndex: null }));
  assert.equal(selectPullRequestDraft(store.getSnapshot(), review).notes[1]?.lineIndex, null);
  assert.deepEqual(selectPullRequestDraft(store.getSnapshot(), { ...review, headOid: "newer-head" }).notes, []);
});

test("review drafts are bounded and invalid text never publishes a change", () => {
  const store = new PreviewStore();
  const review = store.getSnapshot().pullRequests[0]!;
  for (const body of [" ", "x".repeat(2001), "a\u0000b"]) {
    assert.equal(store.addPullRequestNote("preview-mac", review.id, review.headOid, { path: null, body, kind: "comment" }), false);
  }
  for (let i = 0; i < PREVIEW_LIMITS.pullRequestNotes; i++) {
    assert.ok(store.addPullRequestNote("preview-mac", review.id, review.headOid, { path: null, body: `Review ${i}`, kind: "approve" }));
  }
  assert.equal(store.addPullRequestNote("preview-mac", review.id, review.headOid, { path: null, body: "One more", kind: "comment" }), false);
  assert.equal(selectPullRequestDraft(store.getSnapshot(), review).notes.length, PREVIEW_LIMITS.pullRequestNotes);
  assert.equal(selectPullRequest(store.getSnapshot(), "preview-mac", review.id)?.reviewDecision, "required");
});

const input: NewPreviewPullRequest = {
  projectId: "lumen-web", workspaceId: "preview-navigation", provider: "github",
  title: "A focused local review", description: "Explain the workspace transition.", baseBranch: "main",
};
test("sample request creation validates source ownership and keeps project, tab and Git state unchanged", () => {
  const store = new PreviewStore({ now: () => 1000 });
  const snapshot = store.getSnapshot();
  const workspace = store.getWorkspace(input.workspaceId);
  assert.equal(store.createPullRequest("preview-linux", input).ok, false);
  assert.equal(store.createPullRequest("preview-mac", { ...input, workspaceId: "preview-quick-start" }).ok, false);
  assert.equal(store.createPullRequest("preview-mac", { ...input, title: "bad\ntitle" }).ok, false);
  assert.equal(store.createPullRequest("preview-mac", { ...input, baseBranch: "feat/workspace-navigation" }).ok, false);
  assert.equal(store.createPullRequest("preview-mac", { ...input, baseBranch: "../main" }).ok, false);
  assert.equal(store.getSnapshot(), snapshot);
  const result = store.createPullRequest("preview-mac", input);
  assert.ok(result.ok);
  const review = selectPullRequest(store.getSnapshot(), "preview-mac", result.id)!;
  assert.equal(review.isDraft, true);
  assert.equal(review.createdByMe, true);
  assert.equal(review.headBranch, workspace?.branch);
  assert.equal(review.updatedAt, 1000);
  assert.equal(review.mergeRequirementsSatisfied, false);
  assert.equal(store.getSnapshot().tabs, snapshot.tabs);
  assert.equal(store.getWorkspace(input.workspaceId), workspace);
  assert.ok(Object.isFrozen(review.files));
  assert.match(review.activity[0]!.body, /Nothing was published/);
});

test("sample request creation is bounded and reset restores all hosted review state", () => {
  const store = new PreviewStore();
  const before = store.getSnapshot();
  const count = before.pullRequests.length;
  for (let i = count; i < PREVIEW_LIMITS.pullRequests; i++) assert.ok(store.createPullRequest("preview-mac", { ...input, title: `Sample ${i}` }).ok);
  assert.equal(store.createPullRequest("preview-mac", input).ok, false);
  store.reset();
  assert.deepEqual(store.getSnapshot().pullRequests, before.pullRequests);
  assert.deepEqual(store.getSnapshot().pullRequestDrafts, []);
  assert.notEqual(store.getSnapshot().pullRequests, before.pullRequests);
});
