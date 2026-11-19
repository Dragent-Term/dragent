import { useState } from "react";
import { StyleSheet, View } from "react-native";
import { Button, FormField, SegmentedControl } from "../../components";
import { useAsyncAction } from "../../hooks/useAsyncAction";
import { spacing } from "../../theme/tokens";
import { Feedback } from "../shared/Feedback";
import type { ProjectSummary } from "../../preview/types";

export interface PreviewCreateFormProps {
  projects: readonly ProjectSummary[];
  /** Preselects the currently scoped project, if any. */
  initialProjectId: string | null;
  createWorkspace: (
    projectId: string,
    name: string,
    branch: string,
  ) => { ok: true; id: string } | { ok: false; message: string };
  onCreated: (id: string) => void;
  onCancel: () => void;
}

/**
 * Concise inline workspace creation: project scope, workspace name, and its
 * own branch, plus one create action. Failures keep the draft; success hands
 * the new workspace id to the caller so it can open the workspace level.
 */
export function PreviewCreateForm({
  projects,
  initialProjectId,
  createWorkspace,
  onCreated,
  onCancel,
}: PreviewCreateFormProps) {
  const [projectId, setProjectId] = useState<string>(
    initialProjectId && projects.some((project) => project.id === initialProjectId)
      ? initialProjectId
      : projects[0]?.id ?? "",
  );
  const [name, setName] = useState("");
  const [branch, setBranch] = useState("");

  const create = useAsyncAction(async () => {
    if (name.trim().length === 0) throw new Error("Name the workspace first.");
    if (branch.trim().length === 0) throw new Error("Name the branch first.");
    if (!projectId) throw new Error("Choose a project first.");
    const result = createWorkspace(projectId, name.trim(), branch.trim());
    if (!result.ok) throw new Error(result.message);
    onCreated(result.id);
  });

  if (projects.length === 0) {
    return (
      <View style={styles.section}>
        <Feedback message="No projects are available in this preview yet." />
        <Button label="Back" variant="ghost" onPress={onCancel} />
      </View>
    );
  }

  return (
    <View style={styles.section}>
      {projects.length > 1 ? (
        <SegmentedControl
          label="Project"
          options={projects.map((project) => ({ id: project.id, label: project.name, hint: project.name }))}
          value={projects.some((project) => project.id === projectId) ? projectId : projects[0]!.id}
          onChange={setProjectId}
        />
      ) : null}
      <View style={styles.row}>
        <View style={styles.field}>
          <FormField
            label="Workspace name"
            value={name}
            onChangeText={setName}
            autoCapitalize="none"
            autoCorrect={false}
            returnKeyType="next"
            placeholder="quick-fix"
            hint="A local name for this demo workspace."
          />
        </View>
        <View style={styles.field}>
          <FormField
            label="Branch"
            value={branch}
            onChangeText={setBranch}
            autoCapitalize="none"
            autoCorrect={false}
            returnKeyType="done"
            placeholder="fix/quick-fix"
            hint="This workspace's own branch."
            onSubmitEditing={() => { if (name.trim() && branch.trim()) void create.run(); }}
          />
        </View>
      </View>
      <View style={styles.actions}>
        <Button
          label="Create workspace"
          loading={create.pending}
          disabled={!name.trim() || !branch.trim()}
          onPress={() => { void create.run(); }}
        />
        <Button label="Cancel" variant="ghost" onPress={onCancel} />
      </View>
      <Feedback message={create.error} />
    </View>
  );
}

const styles = StyleSheet.create({
  section: { gap: spacing.sm },
  row: { flexDirection: "row", flexWrap: "wrap", alignItems: "flex-end", gap: spacing.sm },
  field: { flexGrow: 1, flexShrink: 1, flexBasis: 200, minWidth: 0 },
  actions: { flexDirection: "row", flexWrap: "wrap", gap: spacing.sm },
});
