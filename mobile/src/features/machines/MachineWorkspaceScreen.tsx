import { useRef, useState } from "react";
import { SectionList, StyleSheet, View } from "react-native";
import { router, useLocalSearchParams } from "expo-router";
import { AppText, Button, EmptyState, LoadingState } from "../../components";
import { ResourceRow } from "../../components/ResourceRow";
import { Screen } from "../../layout";
import { useTheme } from "../../theme";
import { spacing } from "../../theme/tokens";
import { useMachine } from "../../machines/MachineContext";
import {
  isLegacyCatalog,
  projectKey,
  tabsForWorkspace,
  unplacedTabs,
  knownWorkspaceKeys,
  workspaceBranchLabel,
  workspaceKey,
  workspaceProjectKey,
} from "../../machines/machineCatalog";
import { machineCoordinationDetailPath, machineTabPath } from "../../machines/routes";
import { MachineHeader } from "./MachineHeader";
import { Feedback } from "../shared/Feedback";
import { LiveTabRow } from "./MachineRows";
import { WorkspaceMenu } from "../workspaceMenu/WorkspaceMenu";
import { shouldOpenNewTabMenuOnArrival } from "../../machines/newTab";
import { groupWorkspaceTabs } from "../../machines/workspaceTabs";

/**
 * Live workspace: only this workspace's tabs — explicit workspace keys, real
 * kind/agent status and ownership. Legacy tabs without relationship IDs are
 * never guessed into the list; the note below says so honestly.
 */
export function MachineWorkspaceScreen({
  deviceId,
  workspaceId,
}: {
  deviceId: string;
  workspaceId: string;
}) {
  const { theme } = useTheme();
  const machine = useMachine();
  const { catalog } = machine;
  const { newTab: newTabParam } = useLocalSearchParams<{ newTab?: string | string[] }>();

  // A new-tab failure from the menu shows under the header row.
  const [newTabError, setNewTabError] = useState<string | null>(null);
  // Another screen sends the person here with newTab=1: open the choices once.
  const arrival = useRef(shouldOpenNewTabMenuOnArrival(newTabParam, false));

  if (catalog.status === "loading") {
    return (
      <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
        <MachineHeader title="Workspace" onBack={() => router.back()} backLabel="Back" online={null} />
        <LoadingState label="Loading this workspace's tabs…" />
      </View>
    );
  }
  if (catalog.status === "unavailable") {
    return (
      <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
        <MachineHeader title="Workspace" onBack={() => router.back()} backLabel="Back" online={null} />
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
        <MachineHeader title="Workspace" onBack={() => router.back()} backLabel="Back" online={false} />
        <EmptyState
          title="This machine is offline"
          message="Tabs resume when the desktop reconnects."
          actionLabel="Retry"
          onAction={machine.reconnect}
        />
      </View>
    );
  }

  const workspace = catalog.catalog.workspaces.find((entry) => workspaceKey(entry) === workspaceId) ?? null;
  if (!workspace) {
    return (
      <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
        <MachineHeader title="Workspace unavailable" onBack={() => router.back()} backLabel="Back" online />
        <EmptyState
          title="Workspace not found"
          message="This live workspace is not part of that machine's catalog."
          actionLabel="Back to machine"
          onAction={() => router.replace({ pathname: "/machine/[deviceId]", params: { deviceId } })}
        />
      </View>
    );
  }

  const projectName = (() => {
    const key = workspaceProjectKey(workspace);
    if (!key) return null;
    return catalog.catalog.projects.find((entry) => projectKey(entry) === key)?.name ?? null;
  })();
  const tabs = tabsForWorkspace(catalog.catalog, workspace);
  const legacy = isLegacyCatalog(catalog.catalog);
  const unplaced = unplacedTabs(catalog.catalog, knownWorkspaceKeys(catalog.catalog));
  const context = `${projectName ?? "Machine"} · ${workspace.name}`;

  return (
    <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
      <MachineHeader
        title={workspace.name}
        subtitle={`${projectName ?? catalog.machine.name} · ${workspaceBranchLabel(workspace)}`}
        onBack={() => router.back()}
        backLabel={projectName ? `Back to ${projectName}` : "Back"}
        online
      />
      <Screen maxWidth={1040} contentStyle={styles.content}>
        <SectionList
          style={styles.list}
          sections={groupWorkspaceTabs(tabs).map((section) => ({ ...section, data: [...section.data] }))}
          keyExtractor={(tab) => String(tab.id)}
          stickySectionHeadersEnabled={false}
          renderSectionHeader={({ section }) => (
            <View style={styles.sectionHeader}>
              <AppText
                variant="caption"
                tone={section.key === "needs_you" ? "danger" : "textSubtle"}
                style={styles.sectionTitle}
              >
                {section.title}
              </AppText>
              <AppText variant="caption" tone="textSubtle">{section.data.length}</AppText>
            </View>
          )}
          renderItem={({ item: tab }) => (
            <LiveTabRow
              tab={tab}
              context={context}
              onOpen={() => router.push(machineTabPath(deviceId, tab.id))}
            />
          )}
          keyboardShouldPersistTaps="handled"
          contentContainerStyle={[styles.listContent, tabs.length === 0 ? styles.emptyList : undefined]}
          ListHeaderComponent={
            <View style={styles.header}>
              <View style={styles.headerRow}>
                <AppText variant="caption" tone="textMuted" style={styles.headerLabel}>
                  {legacy
                    ? "This machine shares a legacy catalog without tab links, so tabs are not placed here."
                    : `${tabs.length} ${tabs.length === 1 ? "tab" : "tabs"} in this workspace`}
                  {unplaced.length > 0 && !legacy
                    ? ` · ${unplaced.length} unlinked tab${unplaced.length === 1 ? "" : "s"} not shown`
                    : ""}
                </AppText>
                <WorkspaceMenu
                  deviceId={deviceId}
                  workspace={workspace}
                  trigger="button"
                  backTitle={workspace.name}
                  openNewTab={arrival.current}
                  onError={setNewTabError}
                />
              </View>
              {newTabError ? <Feedback message={newTabError} /> : null}
              {workspace.team ? (
                <ResourceRow
                  icon="workflow"
                  title={workspace.team.title}
                  detail={`Coordination · ${workspace.team.lead ? "the lead works here" : workspace.team.task || "a worker's checkout"}`}
                  accessibilityLabel={`Part of the coordination ${workspace.team.title}. Opens the coordination.`}
                  onPress={() => router.push(machineCoordinationDetailPath(deviceId, workspace.team!.id))}
                />
              ) : null}
            </View>
          }
          ListEmptyComponent={
            <EmptyState
              title="No tabs yet"
              message="Open tabs in this workspace appear here with their real kind and agent status."
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
  emptyList: { flexGrow: 1 },
  padded: { gap: 12, padding: 16 },
  header: { gap: spacing.sm },
  headerRow: { flexDirection: "row", alignItems: "center", gap: spacing.sm },
  headerLabel: { flex: 1, minWidth: 0 },
  sectionHeader: { flexDirection: "row", alignItems: "center", justifyContent: "space-between", paddingTop: spacing.md, paddingBottom: spacing.xs },
  sectionTitle: { fontWeight: "600", textTransform: "uppercase", letterSpacing: 0.4 },
});
