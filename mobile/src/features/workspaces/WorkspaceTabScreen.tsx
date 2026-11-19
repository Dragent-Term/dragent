import { ThemedStatusBar } from "../../components/ThemedStatusBar";
import { useCallback, useEffect, useState } from "react";
import { KeyboardAvoidingView, Modal, Platform, ScrollView, StyleSheet, View, useWindowDimensions } from "react-native";
import { SafeAreaView } from "react-native-safe-area-context";
import { router, useFocusEffect } from "expo-router";
import { AgentMark, AgentStateDot, AppText, BrowserMark, Control, EmptyState, Icon, IconButton, TerminalMark, agentStateLabel } from "../../components";
import { TabStrip } from "../../components/TabStrip";
import { PreviewHeader } from "../../preview/PreviewHeader";
import { usePreview } from "../../preview/PreviewContext";
import { previewTabAgentState } from "../../preview/agentStatus";
import { selectPreviewProject, selectPreviewTab, selectPreviewWorkspace } from "../../preview/PreviewStore";
import type { BrowserPage, WorkspaceFile, WorkspaceTask } from "../../preview/types";
import { AGENT_LABEL } from "../preview/PreviewRows";
import { TerminalView } from "../../terminal";
import { AgentConversation } from "../chat";
import { SessionComposer } from "../shared/SessionComposer";
import { PreviewBrowserView } from "../browser/PreviewBrowserView";
import { PreviewTabForm } from "../preview/PreviewTabForm";
import { useTheme } from "../../theme";
import { spacing } from "../../theme/tokens";

type Panel = "files" | "tasks";

const PANEL_TITLES: Record<Panel, string> = {
  files: "Files",
  tasks: "Workspace checklist",
};

function FileList({ files, selected, onSelect }: {
  files: readonly WorkspaceFile[];
  selected: string | null;
  onSelect: (path: string) => void;
}) {
  const { theme } = useTheme();
  if (files.length === 0) {
    return <EmptyState title="No sample files" message="This workspace has no sample files yet." />;
  }
  return (
    <View
      accessibilityLabel="Sample files"
      style={[styles.fileList, { borderColor: theme.colors.border }]}
    >
      {files.map((file) => {
        const active = file.path === selected;
        return (
          <Control
            key={file.path}
            accessibilityRole="button"
            accessibilityState={{ selected: active }}
            accessibilityLabel={`${file.path}${active ? ", selected" : ""}`}
            onPress={() => onSelect(file.path)}
            style={({ pressed, hovered }) => [
              styles.fileRow,
              {
                borderColor: theme.colors.border,
                backgroundColor: active
                  ? theme.colors.surfaceRaised
                  : pressed || hovered
                    ? theme.colors.surfaceRaised
                    : undefined,
              },
            ]}
          >
            <Icon name="folder" size={16} tone={active ? "text" : "textMuted"} />
            <AppText style={styles.fileName} numberOfLines={1}>{file.path}</AppText>
          </Control>
        );
      })}
    </View>
  );
}

function FileSource({ file }: { file: WorkspaceFile | null }) {
  const { theme } = useTheme();
  if (!file) {
    return <EmptyState title="No file selected" message="Choose a sample file to read it here." />;
  }
  return (
    <View style={[styles.plainBlock, { borderColor: theme.colors.border }]}>
      <AppText variant="caption" tone="textMuted" selectable>{file.path}</AppText>
      <AppText variant="caption" selectable style={styles.monospace}>
        {file.content || "This sample file is empty."}
      </AppText>
    </View>
  );
}

function TaskRow({ workspaceId, task, onToggle }: {
  workspaceId: string;
  task: WorkspaceTask;
  onToggle: (workspaceId: string, taskId: string) => void;
}) {
  const { theme } = useTheme();
  const done = task.status === "done";
  return (
    <Control
      accessibilityRole="checkbox"
      accessibilityState={{ checked: done }}
      accessibilityLabel={`${task.title}, ${done ? "done" : "not done"}`}
      accessibilityHint={done ? "Marks this sample task as not done." : "Marks this sample task as done."}
      onPress={() => onToggle(workspaceId, task.id)}
      style={({ pressed, hovered }) => [
        styles.taskRow,
        {
          borderColor: theme.colors.border,
          backgroundColor: pressed || hovered ? theme.colors.surfaceRaised : undefined,
        },
      ]}
    >
      <View
        accessible={false}
        importantForAccessibility="no-hide-descendants"
        style={[
          styles.checkbox,
          {
            borderColor: done ? theme.colors.success : theme.colors.border,
            backgroundColor: done ? theme.colors.surfaceRaised : undefined,
          },
        ]}
      >
        {done ? <Icon name="check" size={16} tone="success" /> : null}
      </View>
      <View style={styles.taskCopy}>
        <AppText style={done ? styles.taskDone : styles.taskTitle}>{task.title}</AppText>
        <AppText variant="caption" tone="textMuted">
          {done ? "Done" : task.status === "doing" ? "Doing" : "To do"}
        </AppText>
      </View>
    </Control>
  );
}

/**
 * Full-height leaf for one open tab inside one workspace. Terminal and agent
 * tabs share the canvas + compact dock; browser tabs render the local page
 * preview with no composer. The composer stays mounted behind auxiliary
 * modals so drafts survive file/change/task inspection, and per-tab keys
 * reset identity whenever the tab changes.
 */
export function WorkspaceTabScreen({ desktopId, projectId, workspaceId, tabId }: {
  desktopId: string;
  projectId: string;
  workspaceId: string;
  tabId: string;
}) {
  const preview = usePreview();
  const { theme } = useTheme();
  const { width, height, fontScale } = useWindowDimensions();
  const [panel, setPanel] = useState<Panel | null>(null);
  const [selectedFile, setSelectedFile] = useState<string | null>(null);
  const [toolsOpen, setToolsOpen] = useState(false);
  const [addOpen, setAddOpen] = useState(false);

  useFocusEffect(useCallback(() => () => {
    setPanel(null);
    setToolsOpen(false);
    setAddOpen(false);
  }, []));

  const desktop = preview.desktops.find((entry) => entry.id === desktopId) ?? null;
  const project = selectPreviewProject(preview, desktopId, projectId);
  const workspaceEntry = selectPreviewWorkspace(preview, desktopId, projectId, workspaceId);
  const tabEntry = selectPreviewTab(preview, desktopId, projectId, workspaceId, tabId);
  const workspace = workspaceEntry ? preview.getWorkspace(workspaceEntry.id) : null;
  const detail = tabEntry ? preview.getTab(tabEntry.id) : null;
  const writers = detail && detail.kind !== "browser" ? preview.getWriters(detail.id) : null;

  const workspaceTabs = preview.tabs.filter((entry) =>
    entry.workspaceId === workspaceId && entry.projectId === projectId,
  );

  useEffect(() => {
    setPanel(null);
    setSelectedFile(null);
    setToolsOpen(false);
    setAddOpen(false);
  }, [tabId, workspaceId]);

  // An open tab keeps marking itself seen, so finished work drops from
  // needs-review back to idle while you look at it. markTabSeen only
  // publishes when something is actually new, so this never loops.
  useEffect(() => {
    if (detail) preview.markTabSeen(detail.id);
  });

  if (!desktop) {
    return (
      <View style={[styles.frame, styles.centered, { backgroundColor: theme.colors.background }]}>
        <EmptyState
          title="Desktop not found"
          message="This sample desktop is unavailable. It may have been reset."
          actionLabel="Back to preview"
          onAction={() => router.replace("/preview")}
        />
      </View>
    );
  }

  if (!project) {
    return (
      <View style={[styles.frame, styles.centered, { backgroundColor: theme.colors.background }]}>
        <EmptyState
          title="Project not found"
          message="This sample project is unavailable. It may have been reset."
          actionLabel="Back to desktop"
          onAction={() => router.replace({ pathname: "/preview/desktop/[desktopId]", params: { desktopId } })}
        />
      </View>
    );
  }

  if (!workspaceEntry || !workspace) {
    return (
      <View style={[styles.frame, styles.centered, { backgroundColor: theme.colors.background }]}>
        <EmptyState
          title="Workspace not found"
          message="This sample workspace is unavailable. It may have been reset."
          actionLabel={`Back to ${project.name} workspaces`}
          onAction={() => router.replace({ pathname: "/preview/desktop/[desktopId]/project/[projectId]", params: { desktopId, projectId } })}
        />
      </View>
    );
  }

  if (!tabEntry || !detail) {
    return (
      <View style={[styles.frame, styles.centered, { backgroundColor: theme.colors.background }]}>
        <EmptyState
          title="Tab not found"
          message="This open tab is unavailable. It may have been reset."
          actionLabel={`Back to ${workspace.name} tabs`}
          onAction={() => router.replace({
            pathname: "/preview/desktop/[desktopId]/project/[projectId]/workspace/[workspaceId]",
            params: { desktopId, projectId, workspaceId },
          })}
        />
      </View>
    );
  }

  const files = workspace.files;
  const activeFile = files.find((file) => file.path === selectedFile) ?? files[0] ?? null;
  const panelTitle = panel ? PANEL_TITLES[panel] : null;
  // Short landscape windows fold the tabs into the header's top row so the
  // canvas keeps full height; portrait keeps a separate tab row.
  const compactBar = width > height && height < 500;
  // Narrow windows and large fonts collapse the four header tools into one
  // More sheet so the workspace title stays readable; every action remains
  // reachable through the sheet.
  const readableWidth = width / Math.max(1, fontScale || 1);
  const toolsOverflow = readableWidth < 430;

  const openPanel = (next: Panel) => {
    setToolsOpen(false);
    setPanel(next);
  };
  const openReview = () => {
    setToolsOpen(false);
    setPanel(null);
    router.push({ pathname: "/preview/desktop/[desktopId]/project/[projectId]/workspace/[workspaceId]/review", params: { desktopId, projectId, workspaceId, tabId } });
  };
  const openAdd = () => {
    setToolsOpen(false);
    setPanel(null);
    setAddOpen(true);
  };
  const handleCreated = (nextId: string) => {
    setAddOpen(false);
    setPanel(null);
    // Keep the leaf screen mounted: changing tabs must preserve keyboard
    // focus and the strip's scroll position.
    router.setParams({ tabId: nextId });
  };

  const toolbar = toolsOverflow ? (
    <IconButton name="more" label="Open workspace tools" selected={toolsOpen || addOpen}
      onPress={() => setToolsOpen(true)} />
  ) : (
    <View style={styles.toolbar} accessibilityRole="toolbar" accessibilityLabel="Workspace tools">
      <IconButton name="folder" label="Open files" selected={panel === "files"} onPress={() => openPanel("files")} />
      <IconButton name="branch" label="Open code review" onPress={openReview} />
      <IconButton name="tasks" label="Open workspace checklist" selected={panel === "tasks"} onPress={() => openPanel("tasks")} />
      <IconButton name="plus" label="New tab" selected={addOpen} onPress={openAdd} />
    </View>
  );

  const tabStrip = (
    <TabStrip
      label={`${workspace.name} tabs`}
      options={workspaceTabs.map((entry) => {
        const state = previewTabAgentState(preview.getTab(entry.id), preview.tabSeen[entry.id] ?? 0);
        const live = state !== "idle" ? `, ${agentStateLabel(state)}` : "";
        return {
          id: entry.id,
          label: entry.title,
          hint: entry.kind === "agent"
            ? `${entry.title}, ${AGENT_LABEL[entry.agent]} agent${live}`
            : `${entry.title}, ${entry.kind}${live}`,
          leading: (
            <View style={styles.tabLeading}>
              <AgentStateDot state={state} />
              {entry.kind === "agent" ? <AgentMark agent={entry.agent} size={13} />
                : entry.kind === "terminal" ? <TerminalMark size={13} />
                : <BrowserMark size={13} />}
            </View>
          ),
        };
      })}
      value={detail.id}
      onChange={(next) => {
        if (next === detail.id) return;
        setPanel(null);
        router.setParams({ tabId: next });
      }}
    />
  );

  const surface = detail.kind === "browser" ? null : detail.terminal.surface;

  return (
    <KeyboardAvoidingView
      style={styles.frame}
      behavior={Platform.OS === "ios" ? "padding" : undefined}
    >
    <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
      <PreviewHeader
        title={workspace.name}
        subtitle={`${project.name} · ${workspace.branch}`}
        onBack={() => router.navigate({
          pathname: "/preview/desktop/[desktopId]/project/[projectId]/workspace/[workspaceId]",
          params: { desktopId, projectId, workspaceId },
        })}
        backLabel={`Back to ${workspace.name} tabs`}
        navigation={compactBar ? tabStrip : undefined}
        actions={toolbar}
      />
      {compactBar ? null : tabStrip}
      <View style={styles.canvas}>
        {detail.kind === "browser" ? (
          <PreviewBrowserView
            key={detail.id}
            state={detail.browser}
            projectName={project.name}
            workspaceName={workspace.name}
            onNavigate={(page: BrowserPage) => preview.navigateBrowser(detail.id, page)}
            onBack={() => preview.goBrowser(detail.id, "back")}
            onForward={() => preview.goBrowser(detail.id, "forward")}
            onReload={() => preview.reloadBrowser(detail.id)}
          />
        ) : (
          detail.kind === "agent" ? <AgentConversation key={detail.id} sessionId={detail.id} state={detail.terminal} /> :
          <TerminalView key={detail.id} presentation="canvas" content="terminal" state={detail.terminal} />
        )}
      </View>
      {detail.kind === "browser" ? null : (
        <View style={styles.dock}>
          <SessionComposer
            key={detail.id}
            sessionId={detail.id}
            mode={detail.kind}
            connected
            role="controller"
            aiStatus={surface?.ai_status ?? null}
            commandRunning={surface?.command_running ?? false}
            writers={writers}
            environment="preview"
            layout="dock"
          />
        </View>
      )}
      <Modal visible={toolsOpen} animationType="fade" transparent onRequestClose={() => setToolsOpen(false)}>
        <ThemedStatusBar />
        <View style={styles.sheetScrim}>
          <View style={[styles.sheet, { backgroundColor: theme.colors.surface, borderColor: theme.colors.border }]}>
            {([
              { id: "files", label: "Files", hint: "Open files", icon: "folder" },
              { id: "review", label: "Workspace changes", hint: "Open workspace changes", icon: "branch" },
              { id: "tasks", label: "Workspace checklist", hint: "Open workspace checklist", icon: "tasks" },
              { id: "add", label: "New tab", hint: "Open the new tab form", icon: "plus" },
            ] as const).map((row) => (
              <Control
                key={row.id}
                accessibilityRole="button"
                accessibilityLabel={row.hint}
                onPress={() => {
                  if (row.id === "add") openAdd();
                  else if (row.id === "review") openReview();
                  else openPanel(row.id);
                }}
                style={({ pressed, hovered }) => [
                  styles.sheetRow,
                  pressed || hovered ? { backgroundColor: theme.colors.surfaceRaised } : null,
                ]}
              >
                <Icon name={row.icon} size={18} />
                <AppText>{row.label}</AppText>
              </Control>
            ))}
            <Control
              accessibilityRole="button"
              accessibilityLabel="Close workspace tools"
              onPress={() => setToolsOpen(false)}
              style={({ pressed, hovered }) => [
                styles.sheetRow,
                pressed || hovered ? { backgroundColor: theme.colors.surfaceRaised } : null,
              ]}
            >
              <Icon name="close" size={18} />
              <AppText>Close</AppText>
            </Control>
          </View>
        </View>
      </Modal>
      <Modal visible={panel !== null} animationType="slide" onRequestClose={() => setPanel(null)}>
        <ThemedStatusBar />
        <SafeAreaView edges={["top", "bottom", "left", "right"]} style={[styles.frame, { backgroundColor: theme.colors.background }]}>
          <PreviewHeader
            title={panelTitle ?? workspace.name}
            subtitle={workspace.name}
            onBack={() => setPanel(null)}
            backLabel={`Back to ${detail.kind === "agent" ? `${AGENT_LABEL[detail.agent]} · ` : ""}${workspace.name}`}
          />
          <ScrollView
            style={styles.panelScroll}
            contentContainerStyle={styles.panelContent}
            keyboardShouldPersistTaps="handled"
            keyboardDismissMode="on-drag"
            contentInsetAdjustmentBehavior="never"
            automaticallyAdjustContentInsets={false}
            automaticallyAdjustKeyboardInsets={false}
          >
            {panel === "files" ? (
              <View key={`${workspace.id}:files`} style={styles.panelSection}>
                <FileList files={files} selected={activeFile?.path ?? null} onSelect={setSelectedFile} />
                <FileSource file={activeFile} />
              </View>
            ) : null}
            {panel === "tasks" ? (
              <View key={`${workspace.id}:tasks`} style={styles.panelSection}>
                {workspace.tasks.length === 0 ? (
                  <EmptyState title="No sample tasks" message="This workspace has no sample tasks yet." />
                ) : (
                  workspace.tasks.map((task) => (
                    <TaskRow key={task.id} workspaceId={workspace.id} task={task} onToggle={preview.toggleTask} />
                  ))
                )}
              </View>
            ) : null}
          </ScrollView>
        </SafeAreaView>
      </Modal>
      <Modal visible={addOpen} animationType="slide" onRequestClose={() => setAddOpen(false)}>
        <ThemedStatusBar />
        <SafeAreaView edges={["top", "bottom", "left", "right"]} style={[styles.frame, { backgroundColor: theme.colors.background }]}>
          <KeyboardAvoidingView
            style={styles.frame}
            behavior={Platform.OS === "ios" ? "padding" : undefined}
          >
            <PreviewHeader
              title="New tab"
              subtitle={workspace.name}
              onBack={() => setAddOpen(false)}
              backLabel={`Back to ${workspace.name}`}
            />
            <ScrollView
              style={styles.panelScroll}
              contentContainerStyle={styles.panelContent}
              keyboardShouldPersistTaps="handled"
              keyboardDismissMode="on-drag"
              contentInsetAdjustmentBehavior="never"
              automaticallyAdjustContentInsets={false}
              automaticallyAdjustKeyboardInsets={false}
            >
              <PreviewTabForm
                workspaceId={workspace.id}
                onCreated={handleCreated}
                onCancel={() => setAddOpen(false)}
              />
            </ScrollView>
          </KeyboardAvoidingView>
        </SafeAreaView>
      </Modal>
    </View>
    </KeyboardAvoidingView>
  );
}

const styles = StyleSheet.create({
  frame: { flex: 1, minHeight: 0 },
  tabLeading: { flexDirection: "row", alignItems: "center", gap: 4, flexShrink: 0 },
  centered: { alignItems: "center", justifyContent: "center" },
  toolbar: { flexDirection: "row", alignItems: "center" },
  canvas: { flex: 1, minHeight: 0 },
  dock: { flexGrow: 0, flexShrink: 0 },
  sheetScrim: { flex: 1, justifyContent: "flex-end", backgroundColor: "rgba(0,0,0,0.4)" },
  sheet: { borderTopWidth: 1, paddingVertical: spacing.sm },
  sheetRow: {
    minHeight: 48,
    flexDirection: "row",
    alignItems: "center",
    gap: spacing.md,
    paddingHorizontal: spacing.lg,
  },
  panelScroll: { flex: 1, minHeight: 0 },
  panelContent: { flexGrow: 1, gap: spacing.md, padding: spacing.md, paddingBottom: spacing.lg },
  panelSection: { gap: spacing.md, flexGrow: 1 },
  plainBlock: { borderWidth: 1, borderRadius: 10, borderCurve: "continuous", gap: spacing.sm, padding: spacing.md, minWidth: 0 },
  fileList: { borderWidth: 1, borderRadius: 10, borderCurve: "continuous", minWidth: 0 },
  fileRow: {
    minHeight: 48,
    flexDirection: "row",
    alignItems: "center",
    gap: spacing.md,
    paddingVertical: spacing.sm,
    paddingHorizontal: spacing.md,
    borderBottomWidth: 1,
    minWidth: 0,
  },
  fileName: { flex: 1, minWidth: 0, fontWeight: "600" },
  taskRow: {
    minHeight: 48,
    borderWidth: 1,
    borderRadius: 10,
    borderCurve: "continuous",
    flexDirection: "row",
    alignItems: "center",
    gap: spacing.md,
    padding: spacing.md,
    minWidth: 0,
  },
  checkbox: {
    width: 28,
    height: 28,
    borderRadius: 8,
    borderWidth: 1,
    alignItems: "center",
    justifyContent: "center",
    flexShrink: 0,
  },
  taskCopy: { flex: 1, minWidth: 0, gap: 2 },
  taskTitle: { fontWeight: "600" },
  taskDone: { fontWeight: "600", textDecorationLine: "line-through" },
  monospace: { fontFamily: Platform.OS === "ios" ? "Menlo" : "monospace" },
});
