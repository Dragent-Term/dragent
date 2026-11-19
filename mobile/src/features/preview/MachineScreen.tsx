import { FlatList, StyleSheet, View } from "react-native";
import { router, useLocalSearchParams } from "expo-router";
import { EmptyState, TabStrip } from "../../components";
import { Screen } from "../../layout";
import { useTheme } from "../../theme";
import { usePreview } from "../../preview/PreviewContext";
import { PreviewHeader } from "../../preview/PreviewHeader";
import { MACHINE_VIEWS, parseMachineView } from "../../preview/machineNavigation";
import { ProjectRow } from "./PreviewRows";
import { CoordinationArea } from "../coordination/CoordinationArea";
import { AutomationArea } from "../automations/AutomationArea";
import { CodeReviewArea } from "../reviews/CodeReviewArea";
import { TasksArea } from "../tasks/TasksArea";

/** The five machine areas follow Dragent desktop. Workspace tabs stay separate. */
export function MachineScreen({ desktopId }: { desktopId: string }) {
  const { theme } = useTheme();
  const preview = usePreview();
  const { view: rawView } = useLocalSearchParams<{ view?: string | string[] }>();
  const view = parseMachineView(rawView);
  const desktop = preview.desktops.find((entry) => entry.id === desktopId);
  const projects = preview.projects.filter((project) => project.desktopId === desktopId);
  return (
    <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
      <PreviewHeader title={desktop?.name ?? "Machine unavailable"} subtitle={desktop?.description ?? ""}
        onBack={() => router.navigate("/preview")} backLabel="Back to machines" />
      {desktop ? <>
        <TabStrip label="Machine sections" options={MACHINE_VIEWS} value={view}
          onChange={(next) => router.setParams({ view: next })} />
        {view === "projects" ? (
          <Screen maxWidth={1040}>
            <FlatList data={projects} keyExtractor={(project) => project.id} style={styles.frame}
              contentContainerStyle={styles.list} renderItem={({ item: project }) => {
                const workspaces = preview.workspaces.filter((workspace) => workspace.projectId === project.id);
                const ids = new Set(workspaces.map((workspace) => workspace.id));
                return <ProjectRow project={project} workspaceCount={workspaces.length}
                  tabCount={preview.tabs.filter((tab) => ids.has(tab.workspaceId)).length}
                  onOpen={(projectId) => router.push({ pathname: "/preview/desktop/[desktopId]/project/[projectId]", params: { desktopId, projectId } })} />;
              }}
              ListEmptyComponent={<EmptyState title="No projects on this machine" message="Projects and their workspaces appear here." />} />
          </Screen>
        ) : view === "coordination" ? <CoordinationArea desktopId={desktopId} />
          : view === "automation" ? <AutomationArea desktopId={desktopId} />
            : view === "review" ? <CodeReviewArea desktopId={desktopId} />
              : <TasksArea />}
      </> : <Screen><EmptyState title="Machine not found" message="Return to the machine list to continue."
        actionLabel="Machines" onAction={() => router.replace("/preview")} /></Screen>}
    </View>
  );
}

const styles = StyleSheet.create({ frame: { flex: 1, minHeight: 0 }, list: { paddingBottom: 24 } });
