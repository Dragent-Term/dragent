import { FlatList, StyleSheet, View } from "react-native";
import { router, useLocalSearchParams } from "expo-router";
import { Button, EmptyState, IconButton, LoadingState, TabStrip, RefreshButton } from "../../components";
import { useTheme } from "../../theme";
import { MACHINE_VIEWS, parseMachineView } from "../../preview/machineNavigation";
import { useMachine } from "../../machines/MachineContext";
import { pairedDesktopName } from "../../transport/e2ee/native";
import {
  knownProjectKeys,
  orphanWorkspaces,
  projectKey,
  tabsForProject,
  tabsForWorkspace,
  workspaceKey,
  workspacesForProject,
} from "../../machines/machineCatalog";
import { machineNewSessionPath, machineProjectPath } from "../../machines/routes";
import { MachineHeader } from "./MachineHeader";
import { countNeeds } from "../../machines/attention";
import { Feedback } from "../shared/Feedback";
import { WorkspaceDirectory } from "./WorkspaceDirectory";
import { parseStatusFilter } from "../../machines/workspaceDirectory";
import { MachineCoordinationSection } from "./MachineCoordinationSection";
import { MachineTasksSection } from "./MachineTasksSection";
import { MachineUsageSection } from "./MachineUsageSection";
import { SecurityScreen } from "../security/SecurityScreen";
import { useSecurityState } from "../security/useSecurityState";
import { unpairDesktop } from "../../transport/e2ee/native";
import { machineWorkspacePath } from "../../machines/routes";

/** The desktop's live areas: every workspace (grouped by project), and agent coordination. */
const LIVE_MACHINE_VIEWS = [
  { id: "projects", label: "Workspaces", hint: "Every workspace on this desktop, grouped by project" },
  { id: "coordination", label: "Coordination", hint: "Agent coordination on this desktop" },
  { id: "tasks", label: "Tasks", hint: "Your organisation's tasks" },
  { id: "usage", label: "Usage", hint: "What the agents on this desktop used, and their plan limits" },
] as const;

/**
 * Live machine: real projects from the catalog subscription plus readonly
 * live agent coordination. The five machine areas follow Dragent desktop;
 * automation, review and tasks honestly report unavailable with no fixture
 * presented as live.
 */
export function MachineScreen({ deviceId }: { deviceId: string }) {
  const { theme } = useTheme();
  const machine = useMachine();
  const { view: rawView, show } = useLocalSearchParams<{ view?: string | string[]; show?: string | string[] }>();
  const rawId = Array.isArray(rawView) ? rawView[0] : rawView;
  // Usage is a live-only area (no sample version), so it isn't a preview view.
  const parsed: string = rawId === "usage" ? "usage" : parseMachineView(rawId);
  // Only areas that show live data are offered; others open the workspaces.
  const view = LIVE_MACHINE_VIEWS.some((entry) => entry.id === parsed) ? parsed : "projects";
  const { catalog } = machine;
  // The name from pairing first: the server knows no names after the E2EE
  // cut-over, and an id is never a useful title.
  const pairedName = pairedDesktopName(deviceId);
  const desktopName = pairedName || catalog.machine?.name || "your desktop";
  const security = useSecurityState({
    secure: machine.socket.secure ?? null,
    socketError: machine.socket.error,
    desktopName,
  });

  const header = (
    <MachineHeader
      title={
        catalog.status === "live"
          ? (catalog.catalog.machine?.name || pairedName || catalog.machine.name || "Desktop")
          : (pairedName ?? "Desktop")
      }
      subtitle={
        catalog.status === "live"
          ? [catalog.catalog.machine?.platform || catalog.machine.platform, catalog.catalog.machine?.version ?? catalog.machine.version]
              .filter(Boolean).map((part, i) => (i === 1 ? `v${part}` : part)).join(" · ")
          : ""
      }
      online={catalog.status === "live" ? catalog.machine.online : null}
      onBack={() => router.navigate("/")}
      backLabel="Back to home"
      actions={
        catalog.status === "live" && !catalog.machine.online ? undefined : (
          <>
            <IconButton name="plus" label="New agent session" onPress={() => router.push(machineNewSessionPath(deviceId))} />
            <RefreshButton
              label="Refresh this desktop"
              onPress={machine.reconnect}
              refreshing={catalog.status === "loading"}
            />
          </>
        )
      }
    />
  );

  if (catalog.status === "loading") {
    return (
      <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
        {header}
        <LoadingState label="Loading this machine's live catalog…" />
      </View>
    );
  }

  if (catalog.status === "unavailable") {
    if (security) {
      return (
        <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
          {header}
          <SecurityScreen
            state={security}
            desktopName={catalog.machine?.name || deviceId}
            onPairAgain={() => router.push("/pair-scan")}
            onRemoveDesktop={() => {
              void unpairDesktop(deviceId).then(() => router.replace("/"));
            }}
            onRetry={machine.reconnect}
          />
        </View>
      );
    }
    return (
      <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
        {header}
        <View style={styles.padded}>
          <Feedback message={catalog.reason} />
          <Button label={machine.socket.sessionStatus === "ended" ? "Start new session" : "Retry"} size="sm" variant="ghost" onPress={machine.reconnect} />
        </View>
      </View>
    );
  }

  if (!catalog.machine.online) {
    return (
      <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
        {header}
        <EmptyState
          title="This machine is offline"
          message="Its live catalog resumes when the desktop reconnects. Controls stay disabled while offline."
          actionLabel="Retry"
          onAction={machine.reconnect}
        />
      </View>
    );
  }

  const projects = catalog.catalog.projects;
  const keys = knownProjectKeys(catalog.catalog);
  const orphans = orphanWorkspaces(catalog.catalog, keys);

  return (
    <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
      {header}
      <TabStrip
        label="Machine sections"
        options={LIVE_MACHINE_VIEWS}
        value={view}
        onChange={(next) => router.setParams({ view: next })}
      />
      {view === "projects" ? (
        <WorkspaceDirectory
          key={`show:${parseStatusFilter(show)}`}
          deviceId={deviceId}
          catalog={catalog.catalog}
          initialStatus={parseStatusFilter(show)}
          header={catalog.warning ? <Feedback message={catalog.warning} /> : undefined}
        />
      ) : view === "coordination" ? (
        <MachineCoordinationSection deviceId={deviceId} />
      ) : view === "tasks" ? (
        <MachineTasksSection deviceId={deviceId} />
      ) : view === "usage" ? (
        <MachineUsageSection />
      ) : (
        <EmptyState
          title={`${MACHINE_VIEWS.find((entry) => entry.id === view)?.label ?? "This area"} isn't available live yet`}
          message="Only workspaces, agent coordination and tasks browse live data today. This area shows nothing rather than sample content."
        />
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  frame: { flex: 1, minHeight: 0 },
  list: { paddingBottom: 24 },
  padded: { gap: 12, padding: 16 },
  warning: { paddingHorizontal: 16, paddingTop: 12 },
  orphans: { paddingTop: 8 },
});
