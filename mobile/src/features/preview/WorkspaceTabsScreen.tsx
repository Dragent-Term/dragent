import { useEffect, useMemo, useRef, useState } from "react";
import { FlatList, StyleSheet, View } from "react-native";
import { router } from "expo-router";
import { AppText, Button, EmptyState } from "../../components";
import { Screen } from "../../layout";
import { useTheme } from "../../theme";
import { spacing } from "../../theme/tokens";
import { usePreview } from "../../preview/PreviewContext";
import { selectPreviewProject, selectPreviewWorkspace } from "../../preview/PreviewStore";
import { PreviewHeader } from "../../preview/PreviewHeader";
import type { PreviewTab } from "../../preview/types";
import { PreviewTabForm } from "./PreviewTabForm";
import { WorkspaceTabRow } from "./PreviewRows";
import { previewTabAgentState } from "../../preview/agentStatus";

interface TabsListRow {
  key: string;
  tab: PreviewTab;
}

/**
 * One workspace: only this workspace's tabs — agent (Dragent built-in, Codex,
 * OpenCode), terminal, and browser — each opening the canonical leaf. The
 * shared add-tab form creates additional tabs; success opens the new leaf by
 * its returned id, never a guessed one.
 */
export function WorkspaceTabsScreen({
  desktopId,
  projectId,
  workspaceId,
}: {
  desktopId: string;
  projectId: string;
  workspaceId: string;
}) {
  const { theme } = useTheme();
  const preview = usePreview();
  const [creating, setCreating] = useState(false);
  const listRef = useRef<FlatList<TabsListRow> | null>(null);

  const desktop = preview.desktops.find((entry) => entry.id === desktopId) ?? null;
  const project = selectPreviewProject(preview, desktopId, projectId);
  const workspace = selectPreviewWorkspace(preview, desktopId, projectId, workspaceId);

  const rows = useMemo<TabsListRow[]>(() => {
    if (!project || !workspace) return [];
    return preview.tabs
      .filter((tab) => tab.workspaceId === workspace.id && tab.projectId === project.id)
      .map((tab) => ({ key: tab.id, tab }));
  }, [preview, project, workspace]);

  // Opening the workspace counts as opening its tabs: finished work drops
  // from needs-review back to idle, once per visit. Finishes that land
  // while the list stays open keep their emerald until the next visit.
  const visitedWorkspace = useRef<string | null>(null);
  useEffect(() => {
    if (!workspace || visitedWorkspace.current === workspace.id) return;
    visitedWorkspace.current = workspace.id;
    for (const tab of preview.tabs) {
      if (tab.workspaceId === workspace.id) preview.markTabSeen(tab.id);
    }
  });

  useEffect(() => {
    if (creating) listRef.current?.scrollToOffset({ offset: 0, animated: true });
  }, [creating]);

  const openTab = (tab: PreviewTab) =>
    router.push({
      pathname: "/preview/desktop/[desktopId]/project/[projectId]/workspace/[workspaceId]/tab/[tabId]",
      params: { desktopId, projectId, workspaceId: tab.workspaceId, tabId: tab.id },
    });

  if (!desktop || !project || !workspace) {
    return (
      <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
        <PreviewHeader
          title="Workspace unavailable"
          onBack={() =>
            router.replace(
              desktop && project
                ? { pathname: "/preview/desktop/[desktopId]/project/[projectId]", params: { desktopId, projectId } }
                : desktop
                  ? { pathname: "/preview/desktop/[desktopId]", params: { desktopId: desktop.id } }
                  : "/preview",
            )
          }
          backLabel={project ? `Back to ${project.name}` : desktop ? `Back to ${desktop.name}` : "Back to desktops"}
        />
        <Screen maxWidth={1040} contentStyle={styles.content}>
          <EmptyState
            title="Workspace not found"
            message="This sample workspace is not part of that project."
            actionLabel="Back to desktops"
            onAction={() => router.replace("/preview")}
          />
        </Screen>
      </View>
    );
  }

  const context = `${project.name} · ${workspace.name}`;

  return (
    <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
      <PreviewHeader
        title={workspace.name}
        subtitle={`${project.name} · ${workspace.branch}`}
        onBack={() =>
          router.navigate({ pathname: "/preview/desktop/[desktopId]/project/[projectId]", params: { desktopId, projectId } })
        }
        backLabel={`Back to ${project.name}`}
      />
      <Screen maxWidth={1040} contentStyle={styles.content}>
        <FlatList
          ref={listRef}
          style={styles.list}
          data={rows}
          keyExtractor={(item) => item.key}
          renderItem={({ item }) => (
            <WorkspaceTabRow
              tab={item.tab}
              context={context}
              status={workspace.status}
              agentState={previewTabAgentState(preview.getTab(item.tab.id), preview.tabSeen[item.tab.id] ?? 0)}
              onOpen={openTab}
            />
          )}
          keyboardShouldPersistTaps="handled"
          contentContainerStyle={[styles.listContent, rows.length === 0 ? styles.emptyList : undefined]}
          ListHeaderComponent={
            <View style={styles.listHeader}>
              <View style={styles.titleRow}>
                <AppText variant="caption" tone="textMuted">Tabs in this workspace</AppText>
                {creating ? null : (
                  <Button label="New tab" size="sm" variant="ghost" onPress={() => setCreating(true)} />
                )}
              </View>
              {creating ? (
                <PreviewTabForm
                  workspaceId={workspace.id}
                  onCreated={(tabId) => {
                    setCreating(false);
                    router.push({
                      pathname: "/preview/desktop/[desktopId]/project/[projectId]/workspace/[workspaceId]/tab/[tabId]",
                      params: { desktopId, projectId, workspaceId: workspace.id, tabId },
                    });
                  }}
                  onCancel={() => setCreating(false)}
                />
              ) : null}
            </View>
          }
          ListEmptyComponent={
            <EmptyState
              title="No tabs yet"
              message="Create an agent, terminal, or browser tab to get started."
              actionLabel="New tab"
              onAction={() => setCreating(true)}
            />
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
