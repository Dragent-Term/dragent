import { ThemedStatusBar } from "../../components/ThemedStatusBar";
import { useState } from "react";
import { KeyboardAvoidingView, Modal, Platform, ScrollView, StyleSheet, View } from "react-native";
import { SafeAreaView } from "react-native-safe-area-context";
import {
  AppText,
  Button,
  Control,
  FormField,
  SegmentedControl,
} from "../../components";
import { rgbaFor, useTheme } from "../../theme";
import { spacing } from "../../theme/tokens";
import { usePreview } from "../../preview/PreviewContext";
import { Feedback } from "../shared/Feedback";

const PROVIDER_OPTIONS = [
  { id: "github", label: "GitHub", hint: "Sample GitHub pull request" },
  { id: "gitlab", label: "GitLab", hint: "Sample GitLab merge request" },
] as const;

const TEMPLATE_OPTIONS = [
  { id: "blank", label: "Blank", hint: "Start empty" },
  { id: "bug", label: "Bug fix", hint: "What broke and the fix" },
  { id: "feature", label: "Feature", hint: "What changed and how to check" },
] as const;

const TEMPLATE_BODY: Record<string, string> = {
  bug: "What broke\n\nFix\n\nHow to check",
  feature: "What changed, why, and how to check it…",
};

type Provider = (typeof PROVIDER_OPTIONS)[number]["id"];

/**
 * Reusable new-request sheet. Creates a LOCAL SAMPLE via
 * `createPullRequest` and opens the result — no real Git calls, no provider
 * requests. Every label says sample so the preview never misleads.
 *
 * A native Modal owns presentation: Android back dismisses via
 * onRequestClose, the safe area and keyboard are handled, and the sheet
 * height is constrained with an internal scroll. The dialog semantics live
 * on the sheet itself (web role); the container stays ungrouped so native
 * screen readers keep every field reachable.
 */
export function NewPullRequestSheet({ desktopId, open, onClose, onCreated }: {
  desktopId: string;
  open: boolean;
  onClose: () => void;
  onCreated: (id: string) => void;
}) {
  const { theme } = useTheme();
  const preview = usePreview();
  const projects = preview.projects.filter((project) => project.desktopId === desktopId);
  const [projectId, setProjectId] = useState(projects[0]?.id ?? "");
  const [provider, setProvider] = useState<Provider>("github");
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");
  const [baseBranch, setBaseBranch] = useState("main");
  const [template, setTemplate] = useState<string>("blank");
  const [peopleOpen, setPeopleOpen] = useState(false);
  const [reviewers, setReviewers] = useState("");
  const [assignees, setAssignees] = useState("");
  const [labels, setLabels] = useState("");
  const [error, setError] = useState<string | null>(null);

  const project = projects.find((entry) => entry.id === projectId) ?? null;
  const workspaces = preview.workspaces.filter((workspace) => workspace.projectId === projectId);
  const [workspaceId, setWorkspaceId] = useState("");
  const workspace = workspaces.find((entry) => entry.id === workspaceId) ?? workspaces[0] ?? null;
  const compareOk = workspace ? baseBranch.trim() !== "" && baseBranch.trim() !== workspace.branch : false;

  const save = () => {
    if (!project || !workspace) {
      setError("Choose a sample project and source workspace first.");
      return;
    }
    if (!title.trim()) {
      setError("Enter a sample title.");
      return;
    }
    const result = preview.createPullRequest(desktopId, {
      projectId: project.id,
      workspaceId: workspace.id,
      provider,
      title: title.trim(),
      description: description.trim(),
      baseBranch: baseBranch.trim() || "main",
    });
    if (result.ok) onCreated(result.id);
    else setError(result.message);
  };

  return (
    <Modal visible={open} animationType="slide" transparent onRequestClose={onClose}>
      <ThemedStatusBar />
      <SafeAreaView edges={["top", "bottom", "left", "right"]} style={[styles.scrim, { backgroundColor: rgbaFor(theme.colors.background, 0.75) }]}>
        <Control
          accessibilityRole="button"
          accessibilityLabel="Dismiss new sample request"
          onPress={onClose}
          style={styles.backdrop}
        />
        <KeyboardAvoidingView
          style={styles.keyboard}
          behavior={Platform.OS === "ios" ? "padding" : undefined}
        >
          <View
            {...(Platform.OS === "web" ? { role: "dialog" as const, accessibilityLabel: "New sample pull request" } : {})}
            style={[styles.sheet, { backgroundColor: theme.colors.surface, borderColor: theme.colors.border }]}
          >
            <ScrollView
              contentContainerStyle={styles.sheetContent}
              keyboardShouldPersistTaps="handled"
              keyboardDismissMode="on-drag"
            >
              <AppText variant="title" accessibilityRole="header">New sample pull request</AppText>
              <AppText variant="caption" tone="textMuted">
                Creates a local sample only. No real pull request opens and no Git command runs.
              </AppText>
              {projects.length > 0 ? (
                <View style={styles.field}>
                  <AppText variant="caption" style={styles.label}>Sample project</AppText>
                  <SegmentedControl
                    label="Sample project"
                    options={projects.map((entry) => ({ id: entry.id, label: entry.name, hint: entry.name }))}
                    value={projectId}
                    onChange={(next) => { setProjectId(next); setWorkspaceId(""); setError(null); }}
                    layout="scroll"
                  />
                </View>
              ) : (
                <Feedback message="This machine has no sample projects yet." />
              )}
              {workspaces.length > 0 ? (
                <View style={styles.field}>
                  <AppText variant="caption" style={styles.label}>Source workspace (sample)</AppText>
                  <SegmentedControl
                    label="Source workspace"
                    options={workspaces.map((entry) => ({ id: entry.id, label: `${entry.name} · ${entry.branch}`, hint: entry.name }))}
                    value={workspace?.id ?? ""}
                    onChange={(next) => { setWorkspaceId(next); setError(null); }}
                    layout="scroll"
                  />
                </View>
              ) : (
                <Feedback message={project ? "The selected project has no sample workspaces." : undefined} />
              )}
              <View style={styles.field}>
                <AppText variant="caption" style={styles.label}>Provider (sample)</AppText>
                <SegmentedControl label="Provider" options={PROVIDER_OPTIONS} value={provider} onChange={setProvider} layout="scroll" />
              </View>
              <View style={styles.compareBox}>
                <AppText variant="caption" tone="textMuted">
                  {workspace ? `Compare ${workspace.branch} → ${baseBranch.trim() || "…"}` : "Choose a source workspace to preview the comparison."}
                </AppText>
                <AppText variant="caption" tone={compareOk ? "success" : "textSubtle"}>
                  {workspace ? (compareOk ? "Branches differ — ready." : "Pick a target branch different from the source.") : ""}
                </AppText>
                <AppText variant="caption" tone="textSubtle">Preview creates drafts only. Not ready for review yet.</AppText>
              </View>
              <FormField label="Title (sample)" value={title} onChangeText={(text) => { setTitle(text); setError(null); }}
                placeholder="What does this sample change?" maxLength={120} autoFocus />
              <FormField label="Target branch (sample)" value={baseBranch} onChangeText={(text) => { setBaseBranch(text); setError(null); }}
                placeholder="main" autoCapitalize="none" autoCorrect={false} maxLength={128}
                hint={workspace ? `Must differ from the source branch ${workspace.branch}.` : undefined} />
              <View style={styles.field}>
                <AppText variant="caption" style={styles.label}>Template (sample)</AppText>
                <SegmentedControl label="Template" options={TEMPLATE_OPTIONS} value={template}
                  onChange={(next) => {
                    setTemplate(next);
                    if (next !== "blank") setDescription(TEMPLATE_BODY[next] ?? "");
                    setError(null);
                  }} layout="scroll" />
              </View>
              <FormField label="Description (sample)" value={description} onChangeText={setDescription}
                multiline maxLength={4000} placeholder="What changed, why, and how to check it…" />
              <View style={styles.field}>
                <Button label={peopleOpen ? "Hide people & labels" : "People & labels (optional)"} size="sm" variant="ghost" onPress={() => setPeopleOpen(!peopleOpen)} />
                {peopleOpen ? (
                  <View style={styles.peopleBox}>
                    <FormField label="Reviewers (comma-separated, sample)" value={reviewers} onChangeText={setReviewers}
                      placeholder="maya, jo" autoCapitalize="none" maxLength={240} hint="Saved to the local sample only." />
                    <FormField label="Assignees (comma-separated, sample)" value={assignees} onChangeText={setAssignees}
                      placeholder="you" autoCapitalize="none" maxLength={240} />
                    <FormField label="Labels (comma-separated, sample)" value={labels} onChangeText={setLabels}
                      placeholder="polish" autoCapitalize="none" maxLength={240} />
                  </View>
                ) : null}
              </View>
              <Feedback message={error} />
              <View style={styles.actions}>
                <Button label="Cancel" variant="ghost" onPress={onClose} />
                <Button label={provider === "gitlab" ? "Create sample merge request" : "Create sample pull request"}
                  disabled={!project || !workspace || !title.trim()} onPress={save} />
              </View>
            </ScrollView>
          </View>
        </KeyboardAvoidingView>
      </SafeAreaView>
    </Modal>
  );
}

const styles = StyleSheet.create({
  scrim: { flex: 1, justifyContent: "flex-end" },
  backdrop: { position: "absolute", top: 0, left: 0, right: 0, bottom: 0 },
  keyboard: { width: "100%", maxHeight: "92%", justifyContent: "flex-end" },
  sheet: {
    width: "100%",
    maxHeight: "100%",
    borderTopWidth: 1,
    borderTopLeftRadius: 16,
    borderTopRightRadius: 16,
  },
  sheetContent: { gap: spacing.md, padding: spacing.lg, paddingBottom: spacing.xl, flexGrow: 1 },
  field: { gap: spacing.xs },
  compareBox: { gap: 2, padding: spacing.sm, borderRadius: 8 },
  peopleBox: { gap: spacing.sm, paddingTop: spacing.xs },
  label: { fontWeight: "500" },
  actions: { flexDirection: "row", flexWrap: "wrap", justifyContent: "flex-end", gap: spacing.sm },
});
