import { useMemo } from "react";
import { FlatList, StyleSheet, View } from "react-native";
import { router } from "expo-router";
import { AppText, Button, EmptyState, confirmAction } from "../../components";
import { Screen } from "../../layout";
import { useTheme } from "../../theme";
import { spacing } from "../../theme/tokens";
import { usePreview } from "../../preview/PreviewContext";
import { PreviewHeader } from "../../preview/PreviewHeader";
import { DesktopRow } from "./PreviewRows";

interface PickerRow {
  key: string;
  desktopId: string;
  projectCount: number;
  workspaceCount: number;
  tabCount: number;
}

/**
 * Entry catalog: every sample desktop with project and workspace counts from
 * data. Always lists — even a single desktop is an explicit choice, never an
 * auto-skip. The header sits outside the padded screen so it runs edge to
 * edge; the list below owns all scrolling.
 */
export function DesktopPickerScreen() {
  const { theme } = useTheme();
  const preview = usePreview();

  const rows = useMemo<PickerRow[]>(
    () =>
      preview.desktops.map((desktop) => {
        const projects = preview.projects.filter((project) => project.desktopId === desktop.id);
        const projectIds = new Set(projects.map((project) => project.id));
        const workspaces = preview.workspaces.filter((workspace) => projectIds.has(workspace.projectId));
        const workspaceIds = new Set(workspaces.map((workspace) => workspace.id));
        return {
          key: desktop.id,
          desktopId: desktop.id,
          projectCount: projects.length,
          workspaceCount: workspaces.length,
          tabCount: preview.tabs.filter((tab) => workspaceIds.has(tab.workspaceId)).length,
        };
      }),
    [preview],
  );

  const confirmReset = () =>
    confirmAction({
      title: "Reset preview?",
      message: "This restores sample workspaces and tasks across the preview.",
      actionLabel: "Reset",
      actionStyle: "destructive",
      onConfirm: () => preview.reset(),
    });

  return (
    <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
      <PreviewHeader title="Desktops" subtitle="Sample desktops · changes stay in this preview" onBack={() => router.replace("/")} backLabel="Home" />
      <Screen maxWidth={1040} contentStyle={styles.content}>
        <FlatList
          style={styles.list}
          data={rows}
          keyExtractor={(item) => item.key}
          renderItem={({ item }) => {
            const desktop = preview.desktops.find((entry) => entry.id === item.desktopId);
            if (!desktop) return null;
            return (
              <DesktopRow
                desktop={desktop}
                projectCount={item.projectCount}
                workspaceCount={item.workspaceCount}
                tabCount={item.tabCount}
                onOpen={(id) => router.push({ pathname: "/preview/desktop/[desktopId]", params: { desktopId: id } })}
              />
            );
          }}
          keyboardShouldPersistTaps="handled"
          contentContainerStyle={[styles.listContent, rows.length === 0 ? styles.emptyList : undefined]}
          ListEmptyComponent={
            <EmptyState title="No desktops in this preview" message="Sample desktops appear here." />
          }
          ListFooterComponent={
            <View style={styles.footer}>
              <Button label="Exit preview" variant="ghost" onPress={() => router.replace("/")} />
              <Button label="Reset preview" variant="ghost" onPress={confirmReset} />
              <AppText variant="caption" tone="textSubtle">Reset restores sample workspaces and tasks across the preview.</AppText>
            </View>
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
  emptyList: { flexGrow: 1 },
  footer: { gap: spacing.md, paddingTop: spacing.xl },
});
