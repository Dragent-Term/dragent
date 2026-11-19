import { useMemo, useState } from "react";
import { Platform, ScrollView, StyleSheet, View, useWindowDimensions } from "react-native";
import { router } from "expo-router";
import {
  AppText,
  Avatar,
  Button,
  Card,
  Control,
  EmptyState,
  FormField,
  Icon,
  SegmentedControl,
  SelectPopover,
  StatusBadge,
  TabStrip,
  type StatusTone,
} from "../../components";
import { useTheme } from "../../theme";
import { spacing } from "../../theme/tokens";
import { usePreview } from "../../preview/PreviewContext";
import { selectMachineTab, selectMachineWorkspaces } from "../../preview/machineSelectors";
import { selectPullRequest, selectPullRequestDraft, pullRequestReadiness } from "../../preview/pullRequestSelectors";
import type { PreviewPullRequest } from "../../preview/pullRequestTypes";
import { Feedback } from "../shared/Feedback";
import { ReviewDiffView, type DiffLineTarget } from "./ReviewDiffView";
import { projectName, reviewReference, timeAgo } from "./pullRequestData";

type DetailTab = "overview" | "files" | "commits" | "checks" | "activity";
type DraftKind = "comment" | "approve" | "request-changes";

const TAB_HINTS: Record<DetailTab, string> = {
  overview: "Readiness, description, and details",
  files: "Changed files and unified diff",
  commits: "Commit history",
  checks: "Check results for the latest commit",
  activity: "Conversation and local drafts",
};

const KIND_OPTIONS = [
  { id: "comment", label: "Comment", hint: "Save a local comment draft" },
  { id: "approve", label: "Approve", hint: "Save a local approval draft" },
  { id: "request-changes", label: "Request changes", hint: "Save a local request-changes draft" },
] as const;

const CHECK_ORDER = ["failed", "running", "pending", "passed", "skipped"] as const;

const STATE_BADGE: Record<string, { label: string; tone: StatusTone }> = {
  open: { label: "Open", tone: "success" },
  closed: { label: "Closed", tone: "danger" },
  merged: { label: "Merged", tone: "accent" },
};

function stateBadge(review: PreviewPullRequest): { label: string; tone: StatusTone } {
  if (review.isDraft && review.state === "open") return { label: "Draft", tone: "warning" };
  return STATE_BADGE[review.state] ?? { label: review.state, tone: "neutral" };
}

function checkTone(status: string): StatusTone {
  if (status === "failed") return "danger";
  if (status === "running" || status === "pending") return "warning";
  if (status === "skipped") return "neutral";
  return "success";
}

function checkLabel(status: string): string {
  if (status === "failed") return "Failed";
  if (status === "running") return "Running";
  if (status === "pending") return "Pending";
  if (status === "skipped") return "Skipped";
  return "Passed";
}

function DraftComposer({ kind, onKindChange, body, onBodyChange, error, onSave, onCancel, cancellable, extendRange, onToggleRange, rangeLabel }: {
  kind: DraftKind;
  onKindChange: (kind: DraftKind) => void;
  body: string;
  onBodyChange: (body: string) => void;
  error: string | null;
  onSave: () => void;
  onCancel: () => void;
  cancellable: boolean;
  extendRange?: boolean;
  onToggleRange?: () => void;
  rangeLabel?: string | null;
}) {
  return (
    <View style={styles.composer}>
      <SegmentedControl label="Draft kind" options={KIND_OPTIONS} value={kind} onChange={onKindChange} layout="scroll" size="sm" />
      {onToggleRange ? (
        <View style={styles.actions}>
          <Button label={extendRange ? "Range: on — tap end line" : "Select a line range"} size="sm" variant="ghost" onPress={onToggleRange} />
          {rangeLabel ? <AppText variant="caption" tone="accent">{rangeLabel}</AppText> : null}
        </View>
      ) : null}
      <FormField
        label="Review draft"
        value={body}
        onChangeText={onBodyChange}
        multiline
        maxLength={2000}
        placeholder="Write a local draft…"
        hint="Local draft only — nothing is published."
        error={error ?? undefined}
        style={styles.draftInput}
      />
      <View style={styles.actions}>
        {cancellable ? <Button label="Cancel" size="sm" variant="ghost" onPress={onCancel} /> : null}
        <Button label="Save local draft" size="sm" disabled={!body.trim()} onPress={onSave} />
      </View>
    </View>
  );
}

/**
 * Shared request detail renderer, mirroring desktop `detail()` +
 * `overview()` / `files()` / `commits()` / `checks()` / `activity()`.
 * Used both by the wide inbox split and the narrow route screen.
 * Provider mutations stay disabled with honest local-preview reasons;
 * approve/request-changes/comment save LOCAL DRAFTS only.
 */
export function PullRequestDetail({ desktopId, reviewId }: { desktopId: string; reviewId: string }) {
  const { theme } = useTheme();
  const preview = usePreview();
  const { height: windowHeight } = useWindowDimensions();
  const compactHeader = windowHeight < 650;
  const [tab, setTab] = useState<DetailTab>("overview");
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [hideViewed, setHideViewed] = useState(false);
  const [pendingTarget, setPendingTarget] = useState<DiffLineTarget | null>(null);
  const [commentOpen, setCommentOpen] = useState(false);
  const [kind, setKind] = useState<DraftKind>("comment");
  const [body, setBody] = useState("");
  const [draftError, setDraftError] = useState<string | null>(null);
  const [feedback, setFeedback] = useState<string | null>(null);
  const [moreOpen, setMoreOpen] = useState(false);
  const [editOpen, setEditOpen] = useState(false);
  const [titleOverride, setTitleOverride] = useState<string | null>(null);
  const [descOverride, setDescOverride] = useState<string | null>(null);
  const [reviewers, setReviewers] = useState<readonly string[] | null>(null);
  const [assignees, setAssignees] = useState<readonly string[] | null>(null);
  const [labels, setLabels] = useState<readonly string[] | null>(null);
  const [addingField, setAddingField] = useState<"Reviewers" | "Assignees" | "Labels" | null>(null);
  const [addValue, setAddValue] = useState("");
  const [mergeOpen, setMergeOpen] = useState(false);
  const [mergeMethod, setMergeMethod] = useState<"merge" | "squash" | "rebase">("squash");
  const [confirmAction, setConfirmAction] = useState<string | null>(null);
  const [split, setSplit] = useState(false);
  const [commitSha, setCommitSha] = useState<string | null>(null);
  const [extendRange, setExtendRange] = useState(false);
  const [draftsOpen, setDraftsOpen] = useState(false);
  const [resolvedNotes, setResolvedNotes] = useState<readonly string[]>([]);
  const [replyTo, setReplyTo] = useState<string | null>(null);
  const [replyBody, setReplyBody] = useState("");
  const toggleResolved = (id: string) => {
    setResolvedNotes((prev) => prev.includes(id) ? prev.filter((v) => v !== id) : [...prev, id]);
  };

  const review = selectPullRequest(preview, desktopId, reviewId);
  const draft = useMemo(() => (review ? selectPullRequestDraft(preview, review) : null), [preview, review]);
  const readiness = useMemo(() => (review ? pullRequestReadiness(review) : null), [review]);

  if (!review || !draft || !readiness) {
    return (
      <View style={styles.unavailable}>
        <EmptyState
          title="Review unavailable"
          message="This sample review is not part of that machine."
        />
      </View>
    );
  }

  const project = projectName(preview, review.projectId);
  const state = stateBadge(review);
  const linkedWorkspace = review.workspaceId
    ? (selectMachineWorkspaces(preview, desktopId).find((entry) => entry.id === review.workspaceId) ?? null)
    : null;
  const activeReviewers = reviewers ?? review.reviewers;
  const activeAssignees = assignees ?? review.assignees;
  const activeLabels = labels ?? review.labels;
  const unresolvedCount = review.reviewDecision === "changes-requested" ? review.activity.filter((e) => e.kind === "review").length || 1 : 0;

  const submitPerson = () => {
    const name = addValue.trim();
    if (!name) return;
    if (addingField === "Reviewers") setReviewers([...activeReviewers, name]);
    else if (addingField === "Assignees") setAssignees([...activeAssignees, name]);
    else if (addingField === "Labels") setLabels([...activeLabels, name]);
    setAddingField(null);
    setAddValue("");
    setFeedback("Saved locally in this preview. Nothing was published.");
  };
  const removePerson = (field: "Reviewers" | "Assignees" | "Labels", name: string) => {
    if (field === "Reviewers") setReviewers(activeReviewers.filter((v) => v !== name));
    else if (field === "Assignees") setAssignees(activeAssignees.filter((v) => v !== name));
    else setLabels(activeLabels.filter((v) => v !== name));
  };
  const stats = {
    files: review.files.length,
    added: review.files.reduce((sum, file) => sum + file.added, 0),
    removed: review.files.reduce((sum, file) => sum + file.removed, 0),
  };
  const activityCount = review.activity.length + draft.notes.length;

  const tabOptions = ([
    { id: "overview", label: "Overview" },
    { id: "files", label: review.files.length > 0 ? `Files ${review.files.length}` : "Files" },
    { id: "commits", label: review.commits.length > 0 ? `Commits ${review.commits.length}` : "Commits" },
    { id: "checks", label: review.checks.length > 0 ? `Checks ${review.checks.length}` : "Checks" },
    { id: "activity", label: activityCount > 0 ? `Activity ${activityCount}` : "Activity" },
  ] as const).map((option) => ({ ...option, hint: TAB_HINTS[option.id] }));

  const openWorkspaceTab = (tabId: string) => {
    const target = selectMachineTab(preview, desktopId, tabId);
    if (!target) {
      setFeedback("That tab is not part of this sample machine.");
      return;
    }
    router.push({
      pathname: "/preview/desktop/[desktopId]/project/[projectId]/workspace/[workspaceId]/tab/[tabId]",
      params: { desktopId, projectId: target.projectId, workspaceId: target.workspaceId, tabId: target.id },
    });
  };

  /** Review with agent opens the linked sample workspace's existing agent tab. Preview only. */
  const reviewWithAgent = () => {
    if (!linkedWorkspace) {
      setFeedback("This sample review links to no workspace, so there is no agent tab to open.");
      return;
    }
    const agentTab = preview.tabs.find((entry) => entry.workspaceId === linkedWorkspace.id && entry.kind === "agent");
    if (!agentTab || !selectMachineTab(preview, desktopId, agentTab.id)) {
      setFeedback("The linked sample workspace has no agent tab in this preview. No model was contacted.");
      return;
    }
    openWorkspaceTab(agentTab.id);
  };

  const openWorkspace = () => {
    if (!linkedWorkspace) {
      setFeedback("This sample review links to no workspace.");
      return;
    }
    router.push({
      pathname: "/preview/desktop/[desktopId]/project/[projectId]/workspace/[workspaceId]",
      params: { desktopId, projectId: linkedWorkspace.projectId, workspaceId: linkedWorkspace.id },
    });
  };

  const saveDraft = (path: string | null, lineIndex: number | null, lineEnd?: number | null) => {
    if (!body.trim()) return;
    const saved = preview.addPullRequestNote(desktopId, review.id, review.headOid, {
      path, body: body.trim(), kind, lineIndex, lineEnd: lineEnd ?? null,
    });
    if (saved) {
      setBody("");
      setDraftError(null);
      setPendingTarget(null);
      setExtendRange(false);
      setFeedback("Saved as a local draft in this preview. Nothing was published to the provider.");
    } else {
      setDraftError("This draft could not be saved locally.");
    }
  };

  const noteTarget = pendingTarget && selectedPath && pendingTarget.path === selectedPath ? pendingTarget : null;
  const rangeLabel = noteTarget && noteTarget.lineIndex !== null && noteTarget.lineEnd != null
    ? `Lines ${Math.min(noteTarget.lineIndex, noteTarget.lineEnd) + 1}–${Math.max(noteTarget.lineIndex, noteTarget.lineEnd) + 1}`
    : null;

  /** Switching files always drops the note target: a note can only save to its own file. */
  const selectFile = (path: string) => {
    setSelectedPath(path);
    setPendingTarget(null);
    setExtendRange(false);
  };

  const pressLine = (path: string, lineIndex: number | null) => {
    setSelectedPath(path);
    setDraftError(null);
    setFeedback(null);
    if (commitSha) {
      setFeedback("Commit comparison is read-only. Choose All changes to comment.");
      return;
    }
    if (extendRange && lineIndex !== null && pendingTarget && pendingTarget.path === path && pendingTarget.lineIndex !== null) {
      if (lineIndex === pendingTarget.lineIndex) {
        setPendingTarget({ path, lineIndex });
      } else {
        setPendingTarget({ path, lineIndex: pendingTarget.lineIndex, lineEnd: lineIndex });
      }
      return;
    }
    setPendingTarget({ path, lineIndex });
  };

  return (
    <View style={styles.frame}>
      <View style={[styles.header, compactHeader ? styles.headerCompact : null, { borderColor: theme.colors.border }]}>
        <View style={styles.identityRow}>
          <StatusBadge label={state.label} tone={state.tone} />
          <AppText variant="caption" tone="textMuted" numberOfLines={1} style={styles.identity}>
            {`${project} / ${reviewReference(review)}`}
          </AppText>
        </View>
        <AppText variant={compactHeader ? "body" : "title"} accessibilityRole="header" numberOfLines={compactHeader ? 1 : undefined} style={styles.title}>{titleOverride ?? review.title}</AppText>
        <View style={styles.branchRow}>
          <AppText variant="caption" tone="textMuted" numberOfLines={1}>
            {`${review.headBranch} → ${review.baseBranch}`}
          </AppText>
          <View style={styles.byline}>
            <Avatar name={review.author} size={16} />
            <AppText variant="caption" tone="textMuted" numberOfLines={1}>{`by ${review.author}`}</AppText>
          </View>
        </View>
        <View style={styles.actions}>
          <Button label="Review with agent" size="sm" variant="ghost" onPress={reviewWithAgent} />
          <Button label="Open workspace" size="sm" variant="ghost" disabled={!linkedWorkspace} onPress={openWorkspace} />
          <SelectPopover label="More actions" caption="REVIEW ACTIONS" triggerLabel="More"
            options={[
              { id: "copy-link", label: "Copy review link" },
              { id: "open-provider", label: `Open on ${review.provider === "gitlab" ? "GitLab" : "GitHub"}` },
              { id: "copy-summary", label: "Copy review summary" },
              { id: "edit", label: "Edit review details" },
            ]}
            value="copy-link" onChange={(id) => {
              setMoreOpen(false);
              if (id === "copy-link") setFeedback("Review link copied locally (preview URL, not a provider URL).");
              else if (id === "open-provider") setFeedback("Provider links are disabled in this local preview.");
              else if (id === "copy-summary") setFeedback(`Summary copied locally: ${project} ${reviewReference(review)} ${titleOverride ?? review.title}.`);
              else if (id === "edit") setEditOpen(true);
            }}
            open={moreOpen} onOpenChange={setMoreOpen} />
        </View>
        {editOpen ? (
          <Card title="Edit review details" subtitle="Local preview only — nothing is published.">
            <FormField label="Title" value={titleOverride ?? review.title}
              onChangeText={setTitleOverride} maxLength={120} />
            <FormField label="Description" value={descOverride ?? review.description}
              onChangeText={setDescOverride} multiline maxLength={4000} />
            <View style={styles.actions}>
              <Button label="Cancel" size="sm" variant="ghost" onPress={() => { setEditOpen(false); setTitleOverride(null); setDescOverride(null); }} />
              <Button label="Save locally" size="sm" onPress={() => { setEditOpen(false); setFeedback("Saved locally in this preview. Nothing was published."); }} />
            </View>
          </Card>
        ) : null}
        {!linkedWorkspace ? (
          <AppText variant="caption" tone="textSubtle">No linked sample workspace for this review.</AppText>
        ) : null}
        <Feedback message={feedback} />
      </View>
      <TabStrip label="Review sections" options={tabOptions} value={tab} onChange={setTab} />
      {tab === "overview" ? (
        <ScrollView style={styles.body} contentContainerStyle={styles.scrollContent} keyboardShouldPersistTaps="handled">
          <Card title={readiness.label} subtitle={readiness.reason} meta={readiness.label} accentDot={readiness.tone}>
            {unresolvedCount > 0 ? (
              <View style={styles.unresolvedRow}>
                <AppText variant="caption" tone="textMuted">
                  {`${unresolvedCount} unresolved thread${unresolvedCount === 1 ? "" : "s"}`}
                </AppText>
                <Button label="Address feedback" size="sm" variant="ghost" onPress={() => setTab("activity")} />
              </View>
            ) : null}
            {readiness.label === "Checks failing" || readiness.label === "Conflicts" ? (
              <Button label="Investigate" size="sm" variant="ghost" onPress={() => setTab("checks")} />
            ) : readiness.label === "Changes requested" || readiness.label === "Review needed" ? (
              unresolvedCount === 0 ? (
                <Button label="Address feedback" size="sm" variant="ghost" onPress={() => setTab("activity")} />
              ) : null
            ) : null}
            {draft.notes.length > 0 ? (
              <AppText variant="caption" tone="textMuted">
                {`${draft.notes.length} local draft note${draft.notes.length === 1 ? "" : "s"} · saved in this preview only, never published.`}
              </AppText>
            ) : null}
          </Card>
          <Card title="Description">
            {(descOverride ?? review.description).trim() ? (
              <AppText selectable>{descOverride ?? review.description}</AppText>
            ) : (
              <AppText tone="textMuted">No description provided.</AppText>
            )}
          </Card>
          <Card title="Details" subtitle={`Updated ${timeAgo(review.updatedAt)}`}
            meta={`${stats.files} files`} accentDot="neutral">
            <View style={styles.statRow} accessible accessibilityLabel={`${stats.files} files, ${stats.added} additions, ${stats.removed} deletions`}>
              <AppText variant="caption" tone="textMuted">{`${stats.files} files`}</AppText>
              <AppText variant="caption" tone="success" style={styles.statNum}>{`+${stats.added}`}</AppText>
              <AppText variant="caption" tone="danger" style={styles.statNum}>{`−${stats.removed}`}</AppText>
            </View>
            <DetailPeople label="Reviewers" values={activeReviewers} empty="None yet"
              onAdd={() => { setAddingField("Reviewers"); setAddValue(""); }} onRemove={(v) => removePerson("Reviewers", v)} />
            <DetailPeople label="Assignees" values={activeAssignees} empty="None yet"
              onAdd={() => { setAddingField("Assignees"); setAddValue(""); }} onRemove={(v) => removePerson("Assignees", v)} />
            <DetailPeople label="Labels" values={activeLabels} empty="None yet" hideAvatar
              onAdd={() => { setAddingField("Labels"); setAddValue(""); }} onRemove={(v) => removePerson("Labels", v)} />
            {addingField ? (
              <View style={styles.addRow}>
                <FormField label={`${addingField.slice(0, -1)} name`} value={addValue} onChangeText={setAddValue}
                  placeholder={addingField === "Labels" ? "Label name" : "Username"} autoCapitalize="none" maxLength={80} />
                <View style={styles.actions}>
                  <Button label="Cancel" size="sm" variant="ghost" onPress={() => { setAddingField(null); setAddValue(""); }} />
                  <Button label="Add locally" size="sm" disabled={!addValue.trim()} onPress={submitPerson} />
                </View>
              </View>
            ) : null}
            <AppText variant="caption" tone="textSubtle">
              {`Sample ${review.provider === "gitlab" ? "merge" : "pull"} request · people and labels save locally only.`}
            </AppText>
          </Card>
          <Card title="Merge options" subtitle="Repository rules apply. Only the reviewed commit can be merged. Merging is disabled in this local preview.">
            <View style={styles.mergeHeader}>
              <AppText variant="caption" tone="textMuted">Merge method</AppText>
              <SegmentedControl label="Merge method"
                options={[
                  { id: "merge", label: "Merge", hint: "Merge commit" },
                  { id: "squash", label: "Squash", hint: "Squash and merge" },
                  { id: "rebase", label: "Rebase", hint: "Rebase and merge" },
                ]}
                value={mergeMethod} onChange={setMergeMethod} layout="scroll" />
            </View>
            {!mergeOpen ? (
              <Button label="Show merge actions" size="sm" variant="ghost" onPress={() => setMergeOpen(true)} />
            ) : (
              <View style={styles.mergeActions}>
                <Button label={review.provider === "gitlab" ? "Merge request… (preview)" : "Merge pull request… (preview)"}
                  size="sm" disabled={review.state !== "open"} onPress={() => setConfirmAction("merge")} />
                <View style={styles.actions}>
                  <Button label="Update from base… (preview)" size="sm" variant="ghost" onPress={() => setFeedback("Branch updates are disabled in this local preview.")} />
                  <Button label={review.isDraft ? "Mark ready (preview)" : "Convert to draft (preview)"} size="sm" variant="ghost" onPress={() => setFeedback("Draft state changes are local-only in this preview.")} />
                  <Button label={review.state === "open" ? (review.provider === "gitlab" ? "Close merge request… (preview)" : "Close pull request… (preview)") : "Reopen (preview)"} size="sm" variant="ghost" onPress={() => setConfirmAction(review.state === "open" ? "close" : "reopen")} />
                </View>
                <Button label="Hide merge actions" size="sm" variant="ghost" onPress={() => { setMergeOpen(false); setConfirmAction(null); }} />
                {confirmAction ? (
                  <View style={styles.confirmBox}>
                    <AppText variant="caption">
                      {confirmAction === "merge" ? `Merge with ${mergeMethod}? Disabled in preview — no provider call will run.`
                        : confirmAction === "close" ? "Close this request? Disabled in preview."
                        : "Reopen this request? Disabled in preview."}
                    </AppText>
                    <View style={styles.actions}>
                      <Button label="Cancel" size="sm" variant="ghost" onPress={() => setConfirmAction(null)} />
                      <Button label="Confirm (preview only)" size="sm" onPress={() => { setConfirmAction(null); setFeedback("Confirmed locally. Nothing was published to the provider."); }} />
                    </View>
                  </View>
                ) : null}
              </View>
            )}
          </Card>
        </ScrollView>
      ) : tab === "files" ? (
        <View style={styles.filesPane}>
          {draft.notes.length > 0 ? (
            <View style={styles.draftsBar}>
              <AppText variant="caption" tone="textMuted">
                {`${draft.notes.length} pending line comment${draft.notes.length === 1 ? "" : "s"} (local drafts)`}
              </AppText>
              <Button label={draftsOpen ? "Hide drafts" : "Review drafts…"} size="sm" variant="ghost" onPress={() => setDraftsOpen(!draftsOpen)} />
            </View>
          ) : null}
          {draftsOpen && draft.notes.length > 0 ? (
            <View style={styles.draftsPanel}>
              <ScrollView style={styles.draftsScroll} keyboardShouldPersistTaps="handled">
                {draft.notes.map((note) => (
                  <View key={note.id} style={[styles.draftNote, { borderColor: theme.colors.border }]}>
                    <AppText variant="caption" tone="accent" numberOfLines={2}>
                      {(note.path ?? "General") + (note.lineIndex != null ? `:${note.lineIndex + 1}${note.lineEnd != null ? `–${note.lineEnd + 1}` : ""}` : "") + ` · ${note.kind}`}
                    </AppText>
                    <AppText selectable>{note.body}</AppText>
                  </View>
                ))}
                <View style={styles.actions}>
                  <Button label="Submit review (preview)" size="sm" onPress={() => setFeedback("Submit is disabled in this local preview. Drafts stay on this device.")} />
                </View>
              </ScrollView>
            </View>
          ) : null}
          <ReviewDiffView
            files={review.files}
            selectedPath={selectedPath}
            onSelectPath={selectFile}
            isViewed={(path) => draft.viewedPaths.includes(path)}
            onToggleViewed={(path, viewed) => {
              if (!preview.markPullRequestFileViewed(desktopId, review.id, review.headOid, path, viewed)) {
                setFeedback("That file's viewed state could not be saved.");
              }
            }}
            hideViewed={hideViewed}
            onToggleHideViewed={() => setHideViewed(!hideViewed)}
            notesForPath={(path) => draft.notes.filter((note) => note.path === path).map((note) => ({ id: note.id, body: note.body }))}
            onLinePress={pressLine}
            pendingTarget={pendingTarget}
            split={split}
            onToggleSplit={() => setSplit(!split)}
            commitSha={commitSha}
            onClearCommit={commitSha ? () => { setCommitSha(null); setFeedback("Showing all changes."); } : undefined}
            onOpenFile={() => setFeedback("Full file view is disabled in this local preview.")}
            compareOptions={review.commits.length > 0 ? [
              { id: "", label: "All changes" },
              ...review.commits.map((c) => ({ id: c.sha, label: `${c.sha.slice(0, 8)} · ${c.title.slice(0, 32)}` })),
            ] : undefined}
            compareValue={commitSha ?? ""}
            onSelectCompare={review.commits.length > 0 ? (v) => { setCommitSha(v || null); setPendingTarget(null); } : undefined}
            composer={noteTarget ? (
              <View style={[styles.inlineComposer, { borderColor: theme.colors.border }]}>
                <AppText variant="caption" tone="accent" numberOfLines={2}>
                  {noteTarget.lineIndex === null
                    ? `Note on ${noteTarget.path}`
                    : rangeLabel ?? `Note on ${noteTarget.path}, diff line ${noteTarget.lineIndex + 1}`}
                </AppText>
                <DraftComposer kind={kind} onKindChange={setKind} body={body} onBodyChange={setBody}
                  error={draftError} onSave={() => saveDraft(noteTarget.path, noteTarget.lineIndex, noteTarget.lineEnd ?? null)}
                  onCancel={() => { setPendingTarget(null); setBody(""); setDraftError(null); setExtendRange(false); }} cancellable
                  extendRange={extendRange} onToggleRange={() => setExtendRange(!extendRange)} rangeLabel={rangeLabel} />
              </View>
            ) : null}
            hint={commitSha ? "Commit comparison is read-only. Choose All changes to comment." : "Tap a diff line to draft a comment. Select Add note for a file-level draft."}
            emptyMessage="Changed files will appear here."
          />
        </View>
      ) : tab === "commits" ? (
        <ScrollView style={styles.body} contentContainerStyle={styles.scrollContent} keyboardShouldPersistTaps="handled">
          <View style={styles.commitHeader}>
            <AppText variant="caption" tone="textMuted">Commit history</AppText>
            {commitSha ? (
              <Button label="All changes" size="sm" variant="ghost" onPress={() => { setCommitSha(null); setTab("files"); }} />
            ) : null}
          </View>
          {review.commits.length === 0 ? (
            <EmptyState title="No commits in this sample" message="Commits for this review will appear here." />
          ) : (
            review.commits.map((commit) => (
              <Card key={commit.sha} title={commit.title} avatarName={commit.author}
                subtitle={`${commit.sha.slice(0, 8)} · ${commit.author} · ${timeAgo(commit.at)}`}>
                <View style={styles.actions}>
                  <Button label={commitSha === commit.sha ? "Viewing in Files" : "Review this commit"} size="sm" variant="ghost"
                    onPress={() => { setCommitSha(commit.sha); setTab("files"); }} />
                  <Button label="Copy SHA" size="sm" variant="ghost" onPress={() => setFeedback(`SHA ${commit.sha.slice(0, 8)} noted locally (preview only).`)} />
                </View>
              </Card>
            ))
          )}
        </ScrollView>
      ) : tab === "checks" ? (
        <ScrollView style={styles.body} contentContainerStyle={styles.scrollContent} keyboardShouldPersistTaps="handled">
          <AppText variant="caption" tone="textMuted">Results for the latest commit. Checks are read-only samples.</AppText>
          {review.checks.length === 0 ? (
            <EmptyState title="No checks reported" message="No checks have been reported for this commit." />
          ) : (
            <>
              <View style={styles.checkCounts}>
                {(["failed", "running", "pending", "passed", "skipped"] as const).map((status) => {
                  const count = review.checks.filter((check) => check.status === status).length;
                  return count > 0 ? <StatusBadge key={status} label={`${count} ${checkLabel(status).toLowerCase()}`} tone={checkTone(status)} /> : null;
                })}
              </View>
              {[...review.checks]
                .sort((a, b) => CHECK_ORDER.indexOf(a.status as (typeof CHECK_ORDER)[number]) - CHECK_ORDER.indexOf(b.status as (typeof CHECK_ORDER)[number]))
                .map((check) => (
                  <Card key={check.id} title={check.name} meta={checkLabel(check.status)} accentDot={checkTone(check.status)}>
                    <View style={styles.actions}>
                      {check.status === "failed" ? (
                        <Button label="View log" size="sm" variant="ghost" onPress={() => setFeedback(`Log for ${check.name} is unavailable in this local preview.`)} />
                      ) : null}
                      {(check.status === "failed" || check.status === "running") ? (
                        <Button label={review.provider === "gitlab" ? "Retry job… (preview)" : "Rerun… (preview)"} size="sm" variant="ghost" onPress={() => setFeedback("Rerun is disabled in this local preview.")} />
                      ) : null}
                      <Button label="Open details ↗" size="sm" variant="ghost" onPress={() => setFeedback("Check details open on the provider; disabled in this local preview.")} />
                    </View>
                  </Card>
                ))}
            </>
          )}
        </ScrollView>
      ) : (
        <View style={styles.activityPane}>
          <ScrollView style={styles.body} contentContainerStyle={styles.scrollContent} keyboardShouldPersistTaps="handled">
            <AppText variant="caption" tone="textMuted">Conversation & review threads</AppText>
            {review.activity.length === 0 && draft.notes.length === 0 ? (
              <EmptyState
                title="No discussion yet"
                message={review.state === "open"
                  ? "Start the conversation with a comment, or select a line in Files to draft a review."
                  : "No comments or reviews yet. You can still save a local draft below."}
              />
            ) : (
              <>
                {review.activity.map((entry) => (
                  <Card key={entry.id} title={entry.author} avatarName={entry.author}
                    subtitle={new Date(entry.at).toLocaleString()}
                    meta={entry.kind === "comment" ? "Comment" : entry.kind === "review" ? "Review" : "System"}
                    accentDot={entry.kind === "system" ? "neutral" : "accent"}>
                    <AppText selectable>{entry.body}</AppText>
                    {entry.kind !== "system" ? (
                      <View style={styles.actions}>
                        <Button label="Reply" size="sm" variant="ghost" onPress={() => {
                          setReplyTo(entry.id); setReplyBody(""); setCommentOpen(true);
                        }} />
                      </View>
                    ) : null}
                  </Card>
                ))}
                {draft.notes.length > 0 ? (
                  <Card title="Local threads (preview)"
                    subtitle="File-pinned drafts with resolve, reply, and view-file. Saved in this preview only."
                    meta={`${draft.notes.length} draft${draft.notes.length === 1 ? "" : "s"}`} accentDot="warning">
                    {draft.notes.map((note) => {
                      const resolved = resolvedNotes.includes(note.id);
                      return (
                        <View key={note.id} style={[styles.draftNote, { borderColor: theme.colors.border }]}>
                          <View style={styles.threadHeader}>
                            <AppText variant="caption" tone={resolved ? "textSubtle" : "accent"} numberOfLines={1} style={styles.threadTitle}>
                              {(note.path ?? "General") + (note.lineIndex != null ? `:${note.lineIndex + 1}${note.lineEnd != null ? `–${note.lineEnd + 1}` : ""}` : "")}
                            </AppText>
                            <StatusBadge label={resolved ? "Resolved" : "Unresolved"} tone={resolved ? "neutral" : "warning"} />
                          </View>
                          <AppText variant="caption" tone="textMuted" numberOfLines={1}>
                            {`${note.kind}${note.path ? ` · ${note.path}` : ""}${note.lineIndex !== null ? ` · diff line ${note.lineIndex + 1}${note.lineEnd != null ? `–${note.lineEnd + 1}` : ""}` : ""}`}
                          </AppText>
                          <AppText selectable>{note.body}</AppText>
                          <View style={styles.actions}>
                            {note.path ? (
                              <Button label="View file" size="sm" variant="ghost" onPress={() => {
                                setSelectedPath(note.path); setTab("files");
                              }} />
                            ) : null}
                            <Button label={resolved ? "Reopen thread" : "Resolve thread"} size="sm" variant="ghost" onPress={() => toggleResolved(note.id)} />
                            <Button label="Reply" size="sm" variant="ghost" onPress={() => {
                              setReplyTo(note.id); setReplyBody(""); setCommentOpen(true);
                            }} />
                          </View>
                          {replyTo === note.id && commentOpen ? (
                            <FormField label="Reply (local preview)" value={replyBody} onChangeText={setReplyBody}
                              multiline maxLength={2000} placeholder="Write a local reply…" />
                          ) : null}
                        </View>
                      );
                    })}
                  </Card>
                ) : null}
              </>
            )}
          <View style={[styles.activityComposer, { borderColor: theme.colors.border, backgroundColor: theme.colors.surface }]}>
            {commentOpen ? (
              <DraftComposer kind={kind} onKindChange={setKind} body={replyTo ? replyBody : body} onBodyChange={replyTo ? setReplyBody : setBody}
                error={draftError} onSave={() => {
                  if (replyTo) {
                    if (!replyBody.trim()) return;
                    const saved = preview.addPullRequestNote(desktopId, review.id, review.headOid, {
                      path: null, body: `Reply to ${replyTo}: ${replyBody.trim()}`, kind, lineIndex: null,
                    });
                    if (saved) {
                      setReplyBody(""); setReplyTo(null); setCommentOpen(false);
                      setFeedback("Reply saved locally. Nothing was published.");
                    } else setDraftError("Reply could not be saved locally.");
                  } else saveDraft(null, null);
                }}
                onCancel={() => { setCommentOpen(false); setBody(""); setReplyBody(""); setReplyTo(null); setDraftError(null); }} cancellable />
            ) : (
              <Button label="Add comment" variant="ghost" onPress={() => { setCommentOpen(true); setReplyTo(null); setDraftError(null); }} />
            )}
          </View>
          </ScrollView>
        </View>
      )}
    </View>
  );
}

function DetailPeople({ label, values, empty, hideAvatar, onAdd, onRemove }: {
  label: string; values: readonly string[]; empty: string; hideAvatar?: boolean;
  onAdd?: () => void; onRemove?: (value: string) => void;
}) {
  const { theme } = useTheme();
  return (
    <View style={styles.peopleRow}>
      <AppText variant="caption" tone="textMuted" style={styles.peopleLabel}>{label}</AppText>
      <ScrollView horizontal showsHorizontalScrollIndicator={false}
        style={styles.peopleScroll} contentContainerStyle={styles.peopleContent}
        keyboardShouldPersistTaps="handled" accessibilityLabel={label}>
        {values.length === 0 ? <AppText tone="textMuted">{empty}</AppText> : null}
        {values.map((person) => (
          <View key={person} style={[styles.personCard, { borderColor: theme.colors.border, backgroundColor: theme.colors.surface }]}>
            {hideAvatar ? null : <Avatar name={person} size={18} />}
            <AppText variant="caption" numberOfLines={1} style={styles.personName}>{person}</AppText>
            {onRemove ? (
              <Control
                accessibilityRole="button"
                accessibilityLabel={`Remove ${person}`}
                onPress={() => onRemove(person)}
                style={({ pressed, hovered }) => [
                  styles.personRemove,
                  hovered ? { backgroundColor: theme.colors.surfaceRaised } : null,
                  pressed ? styles.personRemovePressed : null,
                ]}
              >
                <Icon name="close" size={12} tone="textMuted" />
              </Control>
            ) : null}
          </View>
        ))}
        {onAdd ? (
          <Button label={`Add ${label.toLowerCase().replace(/s$/, "")}`} size="sm" variant="ghost" onPress={onAdd} />
        ) : null}
      </ScrollView>
    </View>
  );
}

const styles = StyleSheet.create({
  frame: { flex: 1, minHeight: 0, gap: 0 },
  unavailable: { flex: 1, padding: spacing.lg, justifyContent: "center" },
  header: { paddingHorizontal: spacing.lg, paddingVertical: spacing.md, gap: spacing.sm, borderBottomWidth: 1 },
  headerCompact: { paddingHorizontal: spacing.md, paddingVertical: spacing.sm, gap: spacing.xs },
  identityRow: { flexDirection: "row", alignItems: "center", gap: spacing.sm },
  identity: { flex: 1, minWidth: 0 },
  title: { fontWeight: "700" },
  branchRow: { flexDirection: "row", flexWrap: "wrap", alignItems: "center", gap: spacing.sm },
  byline: { flexDirection: "row", alignItems: "center", gap: 6, flexShrink: 1, minWidth: 0 },
  actions: { flexDirection: "row", flexWrap: "wrap", gap: spacing.sm },
  body: { flex: 1, minHeight: 0 },
  scrollContent: { gap: spacing.md, padding: spacing.lg, paddingBottom: spacing.xl, flexGrow: 1 },
  filesPane: { flex: 1, minHeight: 0 },
  commitHeader: { flexDirection: "row", alignItems: "center", justifyContent: "space-between", gap: spacing.sm },
  draftsBar: { flexDirection: "row", alignItems: "center", justifyContent: "space-between", gap: spacing.sm, paddingHorizontal: spacing.md, paddingVertical: spacing.xs },
  draftsPanel: { maxHeight: 220, borderBottomWidth: 1, borderColor: "#00000000" },
  draftsScroll: { paddingHorizontal: spacing.md, paddingBottom: spacing.sm, gap: spacing.sm },
  activityPane: { flex: 1, minHeight: 0 },
  activityComposer: { borderTopWidth: 1, padding: spacing.md },
  inlineComposer: { gap: spacing.xs },
  composer: { gap: spacing.sm },
  checkCounts: { flexDirection: "row", flexWrap: "wrap", gap: spacing.md },
  draftNote: { borderTopWidth: 1, paddingTop: spacing.sm, gap: spacing.xs },
  draftInput: {},
  threadHeader: { flexDirection: "row", alignItems: "center", justifyContent: "space-between", gap: spacing.sm },
  threadTitle: { flex: 1, minWidth: 0 },
  unresolvedRow: { flexDirection: "row", alignItems: "center", justifyContent: "space-between", gap: spacing.sm },
  addRow: { gap: spacing.sm },
  mergeHeader: { gap: spacing.xs },
  mergeActions: { gap: spacing.sm },
  confirmBox: { gap: spacing.sm, paddingTop: spacing.sm },
  statRow: { flexDirection: "row", alignItems: "center", gap: 8 },
  statNum: { fontWeight: "600", fontFamily: Platform.OS === "ios" ? "Menlo" : "monospace" },
  peopleRow: { flexDirection: "row", gap: spacing.md, alignItems: "flex-start" },
  peopleLabel: { width: 84, flexShrink: 0, paddingTop: 6 },
  peopleScroll: { flex: 1, minWidth: 0 },
  peopleContent: { flexDirection: "row", alignItems: "center", gap: spacing.xs, paddingVertical: 2, paddingRight: spacing.xs, flexGrow: 0 },
  personCard: {
    flexDirection: "row", alignItems: "center", gap: 6, flexShrink: 0,
    borderWidth: 1, borderRadius: 999, paddingLeft: 6, paddingRight: 4, paddingVertical: 4,
  },
  personName: { flexShrink: 1, minWidth: 0, maxWidth: 140 },
  personRemove: { borderRadius: 999, minWidth: 24, minHeight: 24, alignItems: "center", justifyContent: "center" },
  personRemovePressed: { opacity: 0.7 },
});
