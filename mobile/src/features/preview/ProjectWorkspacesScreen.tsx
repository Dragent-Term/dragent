import { useEffect, useMemo, useRef, useState } from "react";
import { FlatList, StyleSheet, View } from "react-native";
import { router } from "expo-router";
import { AppText, Button, EmptyState, SearchField } from "../../components";
import { Screen } from "../../layout";
import { useTheme } from "../../theme";
import { spacing } from "../../theme/tokens";
import { usePreview } from "../../preview/PreviewContext";
import { selectPreviewProject } from "../../preview/PreviewStore";
import { PreviewHeader } from "../../preview/PreviewHeader";
import type { WorkspaceSummary } from "../../preview/types";
import { PreviewCreateForm } from "./PreviewCreateForm";
import { WorkspaceRow } from "./PreviewRows";

interface WorkspaceListRow {
  key: string;
  workspace: WorkspaceSummary;
  tabCount: number;
}

/**
 * One project: its workspaces as a real navigable level — each row shows the
 * workspace's own name and branch plus tab count/status. Tapping opens the
 * workspace's tabs; creating a workspace opens that workspace level once it
 * exists, never a terminal tab directly.
 */
export function ProjectWorkspacesScreen({ desktopId, projectId }: { desktopId: string; projectId: string }) {
  const { theme } = useTheme();
  const preview = usePreview();
  const [query, setQuery] = useState("");
  const [creating, setCreating] = useState(false);
  const listRef = useRef<FlatList<WorkspaceListRow> | null>(null);

  const desktop = preview.desktops.find((entry) => entry.id === desktopId) ?? null;
  const project = selectPreviewProject(preview, desktopId, projectId);

  const workspaces = useMemo(
    () => (project ? preview.workspaces.filter((workspace) => workspace.projectId === project.id) : []),
    [preview, project],
  );

  const searching = query.trim().length > 0;

  const rows = useMemo<WorkspaceListRow[]>(() => {
    if (!project) return [];
    const q = query.trim().toLowerCase();
    return workspaces
      .filter((workspace) => {
        if (!q) return true;
        return workspace.name.toLowerCase().includes(q) || workspace.branch.toLowerCase().includes(q);
      })
      .map((workspace) => ({
        key: workspace.id,
        workspace,
        tabCount: preview.tabs.filter((tab) => tab.workspaceId === workspace.id).length,
      }));
  }, [preview, project, workspaces, query]);

  useEffect(() => {
    if (creating) listRef.current?.scrollToOffset({ offset: 0, animated: true });
  }, [creating]);

  const openWorkspace = (workspaceId: string) =>
    router.push({
      pathname: "/preview/desktop/[desktopId]/project/[projectId]/workspace/[workspaceId]",
      params: { desktopId, projectId, workspaceId },
    });

  if (!desktop || !project) {
    return (
      <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
        <PreviewHeader
          title="Project unavailable"
          onBack={() => router.replace(desktop ? { pathname: "/preview/desktop/[desktopId]", params: { desktopId: desktop.id } } : "/preview")}
          backLabel={desktop ? `Back to ${desktop.name}` : "Back to desktops"}
        />
        <Screen maxWidth={1040} contentStyle={styles.content}>
          <EmptyState
            title="Project not found"
            message="This sample project is not part of that desktop."
            actionLabel="Back to desktops"
            onAction={() => router.replace("/preview")}
          />
        </Screen>
      </View>
    );
  }

  return (
    <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
      <PreviewHeader
        title={project.name}
        subtitle={desktop.name}
        onBack={() => router.navigate({ pathname: "/preview/desktop/[desktopId]", params: { desktopId } })}
        backLabel={`Back to ${desktop.name}`}
      />
      <Screen maxWidth={1040} contentStyle={styles.content}>
        <FlatList
          ref={listRef}
          style={styles.list}
          data={rows}
          keyExtractor={(item) => item.key}
          renderItem={({ item }) => (
            <WorkspaceRow workspace={item.workspace} tabCount={item.tabCount} onOpen={openWorkspace} />
          )}
          keyboardShouldPersistTaps="handled"
          contentContainerStyle={[styles.listContent, rows.length === 0 ? styles.emptyList : undefined]}
          ListHeaderComponent={
            <View style={styles.listHeader}>
              <View style={styles.titleRow}>
                <AppText variant="caption" tone="textMuted">Workspaces</AppText>
                {creating ? null : (
                  <Button label="New workspace" size="sm" variant="ghost" onPress={() => setCreating(true)} />
                )}
              </View>
              {creating ? (
                <PreviewCreateForm
                  projects={[project]}
                  initialProjectId={project.id}
                  createWorkspace={preview.createWorkspace}
                  onCreated={(id) => {
                    setCreating(false);
                    openWorkspace(id);
                  }}
                  onCancel={() => setCreating(false)}
                />
              ) : null}
              {workspaces.length > 0 || searching ? (
                <SearchField
                  label="Search workspaces"
                  value={query}
                  onChangeText={setQuery}
                  placeholder="Search name or branch…"
                />
              ) : null}
            </View>
          }
          ListEmptyComponent={
            searching ? (
              <EmptyState
                title="No matching workspaces"
                message="Try another name or branch."
                actionLabel="Clear search"
                onAction={() => setQuery("")}
              />
            ) : (
              <EmptyState
                title="No workspaces yet"
                message="Create a workspace to get started."
                actionLabel="New workspace"
                onAction={() => setCreating(true)}
              />
            )
          }
        />
      </Screen>
    </View>
  );
}

const styles = StyleSheet.create({
  frame: { flex: 1 },
  content: { gap: spacing.md },
  list: { flex: 1, minHeight: 0 },
  listContent: { paddingBottom: spacing.xl },
  listHeader: { gap: spacing.md, marginBottom: spacing.md },
  titleRow: { flexDirection: "row", alignItems: "center", justifyContent: "space-between", gap: spacing.sm },
  emptyList: { flexGrow: 1 },
});
