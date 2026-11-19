import { useState } from "react";
import { FlatList, StyleSheet, View } from "react-native";
import { router } from "expo-router";
import { AppText, Button, EmptyState, IconButton, LoadingState, SearchField } from "../../components";
import { Screen } from "../../layout";
import { useTheme } from "../../theme";
import { spacing } from "../../theme/tokens";
import { useMachine } from "../../machines/MachineContext";
import {
  filterWorkspaces,
  isLegacyCatalog,
  projectKey,
  tabsForWorkspace,
  workspaceKey,
  workspacesForProject,
} from "../../machines/machineCatalog";
import { machineHistoryPath, machineNewSessionPath, machineWorkspacePath } from "../../machines/routes";
import { MachineHeader } from "./MachineHeader";
import { countNeeds, atWorkFirst } from "../../machines/attention";
import { Feedback } from "../shared/Feedback";
import { LiveWorkspaceRow } from "./MachineRows";

/**
 * Live project: only this project's workspaces — explicit relationship keys,
 * never array order. Legacy catalogs without relationship IDs are readable
 * but their workspaces are not guessed into this project.
 */
export function MachineProjectScreen({
  deviceId,
  projectId,
}: {
  deviceId: string;
  projectId: string;
}) {
  const { theme } = useTheme();
  const machine = useMachine();
  const { catalog } = machine;
  const [query, setQuery] = useState("");

  if (catalog.status === "loading") {
    return (
      <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
        <MachineHeader title="Project" onBack={() => router.back()} backLabel="Back" online={null} />
        <LoadingState label="Loading this project's workspaces…" />
      </View>
    );
  }
  if (catalog.status === "unavailable") {
    return (
      <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
        <MachineHeader title="Project" onBack={() => router.back()} backLabel="Back" online={null} />
        <View style={styles.padded}>
          <Feedback message={catalog.reason} />
          <Button label="Retry" size="sm" variant="ghost" onPress={machine.reconnect} />
        </View>
      </View>
    );
  }
  if (!catalog.machine.online) {
    return (
      <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
        <MachineHeader title="Project" onBack={() => router.back()} backLabel="Back" online={false} />
        <EmptyState
          title="This machine is offline"
          message="Workspaces resume when the desktop reconnects."
          actionLabel="Retry"
          onAction={machine.reconnect}
        />
      </View>
    );
  }

  const project = catalog.catalog.projects.find((entry) => projectKey(entry) === projectId) ?? null;
  if (!project) {
    return (
      <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
        <MachineHeader
          title="Project unavailable"
          onBack={() => router.replace({ pathname: "/machine/[deviceId]", params: { deviceId } })}
          backLabel="Back to machine"
          online
        />
        <EmptyState
          title="Project not found"
          message="This live project is not part of that machine's catalog."
          actionLabel="Back to machine"
          onAction={() => router.replace({ pathname: "/machine/[deviceId]", params: { deviceId } })}
        />
      </View>
    );
  }

  const workspaces = workspacesForProject(catalog.catalog, project);
  const legacy = isLegacyCatalog(catalog.catalog);
  const searching = query.trim().length > 0;
  // Workspaces whose agents are at work come first, like the desktop sidebar.
  const filtered = atWorkFirst(filterWorkspaces(workspaces, query), (workspace) => countNeeds(tabsForWorkspace(catalog.catalog, workspace)));

  return (
    <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
      <MachineHeader
        title={project.name}
        subtitle={catalog.machine.name}
        onBack={() => router.navigate({ pathname: "/machine/[deviceId]", params: { deviceId } })}
        backLabel={`Back to ${catalog.machine.name}`}
        online
        actions={
          <>
          <IconButton
            name="history"
            label={`Past sessions in ${project.name}`}
            onPress={() => router.push(machineHistoryPath(deviceId, project.path, project.name))}
          />
          <IconButton
            name="plus"
            label={`New agent session in ${project.name}`}
            onPress={() => router.push(machineNewSessionPath(deviceId, { project: project.path }))}
          />
          </>
        }
      />
      <Screen maxWidth={1040} contentStyle={styles.content}>
        <FlatList
          style={styles.list}
          data={filtered}
          keyExtractor={(workspace) => workspaceKey(workspace)}
          renderItem={({ item: workspace }) => (
            <LiveWorkspaceRow
              name={workspace.name}
              branch={workspace.branch ?? "No branch"}
              tabCount={tabsForWorkspace(catalog.catalog, workspace).length}
                    needs={countNeeds(tabsForWorkspace(catalog.catalog, workspace))}
              onOpen={() => router.push(machineWorkspacePath(deviceId, workspaceKey(workspace)))}
            />
          )}
          keyboardShouldPersistTaps="handled"
          contentContainerStyle={[styles.listContent, filtered.length === 0 ? styles.emptyList : undefined]}
          ListHeaderComponent={
            <View style={styles.listHeader}>
              {legacy ? (
                <AppText variant="caption" tone="textMuted">
                  This machine shares a legacy catalog without project links, so workspaces are not grouped here.
                </AppText>
              ) : (
                <AppText variant="caption" tone="textMuted">Workspaces</AppText>
              )}
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
                message={legacy
                  ? "This legacy catalog carries no project links."
                  : "Workspaces in this project appear here."}
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
  emptyList: { flexGrow: 1 },
  padded: { gap: 12, padding: 16 },
});
