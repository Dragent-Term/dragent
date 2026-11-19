import { ThemedStatusBar } from "../../components/ThemedStatusBar";
import { useCallback, useEffect, useState } from "react";
import { ActivityIndicator, Alert, Modal, Pressable, StyleSheet, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { router, useFocusEffect } from "expo-router";
import { AppText, Control, DragentMark, EmptyState, Icon, IconButton } from "../../components";
import { useMachine } from "../../machines/MachineContext";
import { NEW_TAB_MENU_ITEMS } from "../../machines/newTab";
import { useTheme } from "../../theme";
import { radii, spacing } from "../../theme/tokens";
import type { RelayWorkspace } from "../../transport/relayCatalog";
import type { RelayNewTabKind } from "../../transport/relayRpcMethods";
import { MachineHeader } from "../machines/MachineHeader";
import { DiffViewer, type OpenChange } from "./ChangesPanel";
import { SourceControlPanel } from "./SourceControlPanel";
import { CommentSheet, ReviewBar, commentedLines, type CommentTarget } from "./ReviewSheets";
import { useReview } from "../../machines/review";
import { projectKey, tabsForWorkspace, workspaceProjectKey } from "../../machines/machineCatalog";
import { machineHistoryPath, machineNewSessionPath } from "../../machines/routes";
import { isDragentTab } from "../../transport/relayCatalog";
import { CodeViewer } from "./CodeViewer";
import { FilesPanel } from "./FilesPanel";
import { baseName, relativeTo } from "./paths";
import { useNewTab } from "./useNewTab";

type Sheet = "menu" | "newTab";
type Panel = "files" | "changes";

function KindMark({ kind }: { kind: RelayNewTabKind }) {
  if (kind === "dragent_cli") return <DragentMark size={18} />;
  return <Icon name={kind === "browser" ? "browser" : "terminal"} size={18} tone="accent" />;
}

function SheetRow({ label, detail, leading, onPress, disabled = false }: {
  label: string;
  detail?: string;
  leading: React.ReactNode;
  onPress: () => void;
  disabled?: boolean;
}) {
  const { theme } = useTheme();
  return (
    <Control
      accessibilityRole="button"
      accessibilityLabel={detail ? `${label}, ${detail}` : label}
      disabled={disabled}
      onPress={onPress}
      style={({ pressed, hovered }) => [
        styles.sheetRow,
        pressed || hovered ? { backgroundColor: theme.colors.surfaceRaised } : null,
        disabled ? styles.disabled : null,
      ]}
    >
      <View style={styles.sheetMark}>{leading}</View>
      <View style={styles.flex}>
        <AppText>{label}</AppText>
        {detail ? <AppText variant="caption" tone="textSubtle" numberOfLines={1}>{detail}</AppText> : null}
      </View>
    </Control>
  );
}

/**
 * The workspace menu: Files, Source control, New tab and Close.
 *
 * `trigger="button"` is the labelled "Menu" button on the workspace screen;
 * `trigger="dots"` is the ⋯ button in a tab's header. Both open the same
 * sheet and do the same things:
 *
 * - **Files** browses the workspace's folder on the desktop and opens a file
 *   in a read-only code view.
 * - **Source control** lists the changes (stage, unstage, discard), commits,
 *   pushes, switches branch, opens a pull request and shows history.
 * - **New tab** opens a terminal, browser or Dragent CLI tab on the desktop
 *   and goes to it.
 *
 * `openNewTab` opens the new-tab choices on arrival (once).
 */
export function WorkspaceMenu({ deviceId, workspace, trigger, backTitle, replaceOnNewTab = false, openNewTab = false, newTabSignal = 0, onError }: {
  deviceId: string;
  workspace: RelayWorkspace;
  trigger: "button" | "dots";
  /** What the panels' Back button returns to ("Back to …"). */
  backTitle: string;
  replaceOnNewTab?: boolean;
  openNewTab?: boolean;
  /** Bumped by the screen (its "+" button) to open the new-tab choices. */
  newTabSignal?: number;
  /** Where a new-tab failure is shown (the screen owns that line). */
  onError?: (message: string | null) => void;
}) {
  const { theme } = useTheme();
  const insets = useSafeAreaInsets();
  const { rpc } = useMachine();
  const newTab = useNewTab(deviceId, workspace, replaceOnNewTab);
  const [sheet, setSheet] = useState<Sheet | null>(null);
  const [panel, setPanel] = useState<Panel | null>(null);
  const [file, setFile] = useState<string | null>(null);
  const [change, setChange] = useState<OpenChange | null>(null);
  const [commentOn, setCommentOn] = useState<CommentTarget | null>(null);
  const review = useReview(workspace.path);
  const { catalog } = useMachine();
  // The project this workspace belongs to, by folder, for a new session in it.
  const sessionProject =
    catalog.status === "live"
      ? (catalog.catalog.projects.find((project) => projectKey(project) === workspaceProjectKey(workspace))?.path ?? null)
      : null;
  // The workspace's agents, for sending a review.
  const agents = catalog.status === "live"
    ? tabsForWorkspace(catalog.catalog, workspace).filter((tab) => tab.kind === "agent" || (tab.agent_kind ?? "") !== "" || isDragentTab(tab))
    : [];

  // Leaving the screen closes everything it opened.
  useFocusEffect(useCallback(() => () => {
    setSheet(null);
    setPanel(null);
    setFile(null);
    setChange(null);
  }, []));

  useEffect(() => {
    if (openNewTab && newTab.available) setSheet("newTab");
  }, [openNewTab, newTab.available]);
  useEffect(() => {
    if (newTabSignal > 0 && newTab.available) {
      newTab.clearError();
      setSheet("newTab");
    }
    // Only a new signal opens it; availability changing doesn't.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [newTabSignal]);

  // The screen shows the failure when it has a place for it; otherwise say it here.
  const { error: newTabError, clearError } = newTab;
  useEffect(() => {
    if (onError) {
      onError(newTabError);
    } else if (newTabError) {
      Alert.alert("Couldn't open the tab", newTabError);
      clearError();
    }
  }, [newTabError, onError, clearError]);

  const openPanel = (next: Panel) => {
    setSheet(null);
    setFile(null);
    setChange(null);
    setPanel(next);
  };
  const pick = (kind: RelayNewTabKind) => {
    setSheet(null);
    newTab.create(kind);
  };
  const closePanel = () => {
    setPanel(null);
    setFile(null);
    setChange(null);
  };
  // Back steps out of the open file or diff first, then out of the panel.
  const back = () => {
    if (file !== null) setFile(null);
    else if (change !== null) setChange(null);
    else closePanel();
  };

  const root = workspace.path;
  const detail = file !== null ? file : change !== null ? `${root}/${change.change.path}` : null;
  const panelTitle = detail !== null ? baseName(detail) : panel === "files" ? "Files" : "Source control";
  const panelSubtitle = detail !== null
    ? (relativeTo(root, detail) ?? detail)
    : workspace.name;
  const panelBack = detail !== null ? (panel === "files" ? "Back to files" : "Back to changes") : `Back to ${backTitle}`;

  return (
    <>
      {trigger === "dots" ? (
        // One tap to the files and source control; the rest in the menu.
        <View style={styles.toolbar}>
          <IconButton
            name="folder"
            label="Files"
            selected={panel === "files"}
            onPress={() => openPanel("files")}
          />
          <IconButton
            name="branch"
            label="Source control"
            selected={panel === "changes"}
            onPress={() => openPanel("changes")}
          />
          <IconButton
            name="more"
            label="Open workspace menu"
            selected={sheet !== null}
            onPress={() => setSheet("menu")}
          />
        </View>
      ) : (
        <Control
          accessibilityRole="button"
          accessibilityLabel="Workspace menu"
          accessibilityState={{ expanded: sheet !== null, busy: newTab.creating }}
          disabled={newTab.creating}
          onPress={() => {
            newTab.clearError();
            setSheet("menu");
          }}
          style={({ pressed, hovered }) => [
            styles.trigger,
            {
              borderColor: sheet !== null ? theme.colors.accent : theme.colors.border,
              backgroundColor: pressed || hovered ? theme.colors.surfaceRaised : theme.colors.surface,
            },
          ]}
        >
          {newTab.creating ? (
            <ActivityIndicator accessible={false} color={theme.colors.textMuted} size="small" />
          ) : (
            <Icon name="more" size={15} tone="text" />
          )}
          <AppText variant="caption" style={styles.triggerLabel}>{newTab.creating ? "Opening…" : "Menu"}</AppText>
        </Control>
      )}

      <Modal visible={sheet !== null} animationType="fade" transparent onRequestClose={() => setSheet(null)}>
        <ThemedStatusBar />
        <View style={styles.scrim}>
          <Pressable accessibilityRole="button" accessibilityLabel="Close menu" style={StyleSheet.absoluteFill} onPress={() => setSheet(null)} />
          <View
            style={[
              styles.sheet,
              { backgroundColor: theme.colors.surface, borderColor: theme.colors.border, paddingBottom: Math.max(insets.bottom, spacing.sm) },
            ]}
          >
            <View style={[styles.grabber, { backgroundColor: theme.colors.border }]} />
            {sheet === "newTab" ? (
              <>
                <AppText variant="caption" tone="textMuted" style={styles.sheetTitle}>New tab in {workspace.name}</AppText>
                {NEW_TAB_MENU_ITEMS.map((item) => (
                  <SheetRow
                    key={item.kind}
                    label={item.title}
                    detail={item.detail}
                    leading={<KindMark kind={item.kind} />}
                    disabled={!rpc}
                    onPress={() => pick(item.kind)}
                  />
                ))}
                <SheetRow label="Back" leading={<Icon name="arrow-left" size={18} />} onPress={() => setSheet("menu")} />
              </>
            ) : (
              <>
                <AppText variant="caption" tone="textMuted" style={styles.sheetTitle}>{workspace.name}</AppText>
                <SheetRow label="Files" detail="Browse and read this workspace's files" leading={<Icon name="folder" size={18} />} onPress={() => openPanel("files")} />
                <SheetRow label="Source control" detail="Changes, commit, push, pull request, history" leading={<Icon name="branch" size={18} />} onPress={() => openPanel("changes")} />
                <SheetRow
                  label="New agent session"
                  detail="An installed agent on a task, in this workspace"
                  leading={<Icon name="agent" size={18} />}
                  disabled={!sessionProject}
                  onPress={() => {
                    setSheet(null);
                    if (sessionProject) router.push(machineNewSessionPath(deviceId, { project: sessionProject, workspace: workspace.path }));
                  }}
                />
                <SheetRow
                  label="Past sessions"
                  detail="Earlier agent conversations here: continue or delete"
                  leading={<Icon name="history" size={18} />}
                  onPress={() => {
                    setSheet(null);
                    router.push(machineHistoryPath(deviceId, workspace.path, workspace.name));
                  }}
                />
                <SheetRow
                  label="New tab"
                  detail={newTab.available ? "Terminal, browser or Dragent CLI" : "Not available for this workspace"}
                  leading={<Icon name="plus" size={18} />}
                  disabled={!newTab.available}
                  onPress={() => setSheet("newTab")}
                />
                <SheetRow label="Close" leading={<Icon name="close" size={18} />} onPress={() => setSheet(null)} />
              </>
            )}
          </View>
        </View>
      </Modal>

      <Modal visible={panel !== null} animationType="slide" onRequestClose={back}>
        <ThemedStatusBar />
        {/* A modal is its own window: the safe-area view measures nothing in
            it, so the header slid under the status bar. The insets come from
            the screen behind and are applied by hand. */}
        <View
          style={[
            styles.frame,
            {
              backgroundColor: theme.colors.background,
              paddingTop: insets.top,
              paddingBottom: insets.bottom,
              paddingLeft: insets.left,
              paddingRight: insets.right,
            },
          ]}
        >
          <MachineHeader title={panelTitle} subtitle={panelSubtitle} onBack={back} backLabel={panelBack} online />
          {!rpc ? (
            <EmptyState title="Not connected" message="Files and changes come from your desktop. They show again when it reconnects." />
          ) : !root.startsWith("/") ? (
            <EmptyState title="Not available here" message="This workspace's folder isn't one the phone can read." />
          ) : panel === "files" ? (
            <>
              {/* The browser stays mounted under the open file, so Back returns to the same folder. */}
              <View style={file !== null ? styles.hidden : styles.frame}>
                <FilesPanel rpc={rpc} root={root} onOpen={setFile} />
              </View>
              {file !== null ? <CodeViewer rpc={rpc} path={file} /> : null}
            </>
          ) : panel === "changes" ? (
            <>
              <View style={change !== null ? styles.hidden : styles.frame}>
                <SourceControlPanel rpc={rpc} root={root} onOpen={setChange} />
              </View>
              {change !== null ? (
                <DiffViewer
                  rpc={rpc}
                  root={root}
                  open={change}
                  onComment={(row) => setCommentOn({ file: change.change.path, row })}
                  commentedLines={commentedLines(review, change.change.path)}
                />
              ) : null}
              <ReviewBar rpc={rpc} root={root} agents={agents} />
              <CommentSheet root={root} target={commentOn} onClose={() => setCommentOn(null)} />
            </>
          ) : null}
        </View>
      </Modal>
    </>
  );
}

const styles = StyleSheet.create({
  frame: { flex: 1, minHeight: 0 },
  hidden: { display: "none" },
  flex: { flex: 1, minWidth: 0 },
  disabled: { opacity: 0.45 },
  toolbar: { flexDirection: "row", alignItems: "center", gap: 2 },
  trigger: {
    minHeight: 32,
    flexDirection: "row",
    alignItems: "center",
    gap: 6,
    borderWidth: 1,
    borderRadius: radii.md,
    paddingHorizontal: 10,
    paddingVertical: 4,
  },
  triggerLabel: { fontWeight: "500" },
  scrim: { flex: 1, justifyContent: "flex-end", backgroundColor: "rgba(0,0,0,0.45)" },
  sheet: {
    borderTopWidth: StyleSheet.hairlineWidth,
    borderTopLeftRadius: radii.xl,
    borderTopRightRadius: radii.xl,
    paddingTop: spacing.sm,
  },
  grabber: { alignSelf: "center", width: 36, height: 4, borderRadius: 2, marginBottom: spacing.xs },
  sheetTitle: { paddingHorizontal: spacing.lg, paddingVertical: spacing.xs, fontWeight: "600" },
  sheetRow: {
    minHeight: 52,
    flexDirection: "row",
    alignItems: "center",
    gap: spacing.md,
    paddingHorizontal: spacing.lg,
    paddingVertical: 6,
  },
  sheetMark: { width: 22, alignItems: "center" },
});
