import { useState } from "react";
import { KeyboardAvoidingView, Platform, StyleSheet, View } from "react-native";
import { router } from "expo-router";
import { AppText, Button, EmptyState, FormField } from "../../components";
import { useTheme } from "../../theme";
import { spacing } from "../../theme/tokens";
import { PreviewHeader } from "../../preview/PreviewHeader";
import { usePreview } from "../../preview/PreviewContext";
import { selectPreviewTab, selectPreviewWorkspace } from "../../preview/PreviewStore";
import { ReviewDiffView, type DiffLineTarget } from "./ReviewDiffView";

/**
 * Workspace-local changes. Title stays "Workspace changes" on its existing
 * route with the existing notes/progress APIs and returnTabId behavior.
 * Diffs render through the shared ReviewDiffView; viewed/notes state is the
 * workspace-local review store, fully separate from provider review drafts.
 */
export function WorkspaceReviewScreen({ desktopId, projectId, workspaceId, returnTabId = "" }: {
  desktopId: string; projectId: string; workspaceId: string; returnTabId?: string;
}) {
  const { theme } = useTheme();
  const preview = usePreview();
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [hideViewed, setHideViewed] = useState(false);
  const [pendingTarget, setPendingTarget] = useState<DiffLineTarget | null>(null);
  const [draft, setDraft] = useState("");
  const [noteError, setNoteError] = useState<string | null>(null);

  const summary = selectPreviewWorkspace(preview, desktopId, projectId, workspaceId);
  const workspace = summary ? preview.getWorkspace(summary.id) : null;
  const review = preview.reviews.find((entry) => entry.workspaceId === workspace?.id);
  const reviewed = new Set(review?.reviewedPaths ?? []);
  const returnTab = selectPreviewTab(preview, desktopId, projectId, workspaceId, returnTabId);
  const back = () => returnTab
    ? router.navigate({ pathname: "/preview/desktop/[desktopId]/project/[projectId]/workspace/[workspaceId]/tab/[tabId]", params: { desktopId, projectId, workspaceId, tabId: returnTab.id } })
    : router.navigate({ pathname: "/preview/desktop/[desktopId]", params: { desktopId, view: "review" } });

  const noteTarget = pendingTarget && selectedPath && pendingTarget.path === selectedPath ? pendingTarget : null;

  /** Switching files always drops the note target: a note can only save to its own file. */
  const selectFile = (path: string) => {
    setSelectedPath(path);
    setPendingTarget(null);
  };

  const saveNote = () => {
    if (!noteTarget || !workspace || !draft.trim()) return;
    if (preview.addReviewNote(desktopId, workspace.id, noteTarget.path, draft.trim())) {
      setDraft("");
      setNoteError(null);
      setPendingTarget(null);
    } else {
      setNoteError("This note could not be saved. Each workspace supports up to 40 local notes.");
    }
  };

  return (
    <KeyboardAvoidingView
      style={[styles.fill, { backgroundColor: theme.colors.background }]}
      behavior={Platform.OS === "ios" ? "padding" : undefined}
    >
      <PreviewHeader
        title="Workspace changes"
        subtitle={workspace ? `${workspace.name} · ${workspace.branch}` : ""}
        onBack={back}
        backLabel={returnTab ? `Back to ${workspace?.name ?? "workspace"}` : "Back to code review"}
      />
      {!workspace ? (
        <View style={styles.empty}>
          <EmptyState
            title="Review unavailable"
            message="This workspace does not belong to the selected machine and project."
            actionLabel="Machines"
            onAction={() => router.replace("/preview")}
          />
        </View>
      ) : (
        <>
          <View style={[styles.summary, { borderColor: theme.colors.border }]}>
            <AppText variant="caption" tone="textMuted" numberOfLines={1} style={styles.summaryText}>
              {`${reviewed.size}/${workspace.changes.length} files reviewed · ${review?.notes.length ?? 0} notes · local only`}
            </AppText>
            <Button label="Open workspace" variant="ghost" size="sm" onPress={() => router.push({
              pathname: "/preview/desktop/[desktopId]/project/[projectId]/workspace/[workspaceId]",
              params: { desktopId, projectId, workspaceId },
            })} />
          </View>
          <View style={styles.diffWrap}>
            <ReviewDiffView
              files={workspace.changes}
              selectedPath={selectedPath}
              onSelectPath={selectFile}
              isViewed={(path) => reviewed.has(path)}
              onToggleViewed={(path, viewed) => { preview.setReviewFileReviewed(desktopId, workspace.id, path, viewed); }}
              hideViewed={hideViewed}
              onToggleHideViewed={() => setHideViewed(!hideViewed)}
              notesForPath={(path) => review?.notes.filter((note) => note.path === path).map((note) => ({ id: note.id, body: note.body })) ?? []}
              onLinePress={(path, lineIndex) => { setSelectedPath(path); setPendingTarget({ path, lineIndex }); setNoteError(null); }}
              pendingTarget={pendingTarget}
              composer={noteTarget ? (
                <View style={[styles.inlineComposer, { borderColor: theme.colors.border }]}>
                  <AppText variant="caption" tone="accent" numberOfLines={2}>
                    {`Note on ${noteTarget.path}`}
                  </AppText>
                  <FormField
                    label="Review note"
                    value={draft}
                    onChangeText={(text) => { setDraft(text); setNoteError(null); }}
                    multiline
                    maxLength={2000}
                    placeholder="Leave a note about this file…"
                    hint="Notes attach to the file and stay in this preview."
                    error={noteError ?? undefined}
                  />
                  <View style={styles.noteActions}>
                    <Button label="Cancel" size="sm" variant="ghost"
                      onPress={() => { setPendingTarget(null); setDraft(""); setNoteError(null); }} />
                    <Button label="Save note locally" size="sm" disabled={!draft.trim()} onPress={saveNote} />
                  </View>
                </View>
              ) : null}
              hint="Tap a diff line to draft a note. Select Add note for a file-level note."
              emptyMessage="Workspace changes will appear here."
            />
          </View>
        </>
      )}
    </KeyboardAvoidingView>
  );
}

const styles = StyleSheet.create({
  fill: { flex: 1, minHeight: 0 },
  empty: { flex: 1, padding: spacing.lg, justifyContent: "center" },
  summary: {
    paddingHorizontal: spacing.lg, paddingVertical: spacing.sm, gap: spacing.sm,
    flexDirection: "row", flexWrap: "wrap", alignItems: "center",
    justifyContent: "space-between", borderBottomWidth: 1,
  },
  summaryText: { flexGrow: 1, flexShrink: 1, flexBasis: 200 },
  diffWrap: { flex: 1, minHeight: 0 },
  inlineComposer: { gap: spacing.xs },
  noteActions: { flexDirection: "row", flexWrap: "wrap", justifyContent: "flex-end", gap: spacing.sm },
});
