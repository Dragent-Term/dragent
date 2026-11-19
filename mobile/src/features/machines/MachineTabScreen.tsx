import { ScrollView, StyleSheet, View } from "react-native";
import { router } from "expo-router";
import { AppText, Button, EmptyState, LoadingState, Section, StatusBadge } from "../../components";
import { ScrollScreen } from "../../layout";
import { spacing } from "../../theme/tokens";
import { useTheme } from "../../theme";
import { useMachine } from "../../machines/MachineContext";
import { projectKey, tabsForWorkspace, workspaceKey, workspaceProjectKey } from "../../machines/machineCatalog";
import { dragentActivity, isDragentTab } from "../../transport/relayCatalog";
import { MachineHeader } from "./MachineHeader";
import { WorkspaceTabStrip, WorkspaceTools } from "./WorkspaceTools";
import { Feedback } from "../shared/Feedback";
import { AgentBlock, dragentActivityBlocks } from "../../terminal";
import { CompanionTerminalView } from "../../terminal/companion/CompanionTerminalView";
import { BrowserTabView } from "../browserTab/BrowserTabView";
import { DragentAgentView } from "../agent/DragentAgentView";
import { rememberResume, type ResumePoint } from "../../machines/resume";
import { machineWorkspacePath } from "../../machines/routes";
import { useCallback, useEffect, useState, type ReactNode } from "react";
import { CliConversationView } from "../agent/CliConversationView";
import { useTabSeen } from "./useTabSeen";
import { ViewSwitch } from "../chat/ChatParts";
import { TerminalComposer } from "../chat/TerminalComposer";
import type { RelayTab } from "../../transport/relayCatalog";

/**
 * Agents whose conversation the desktop can read from their own records. A
 * desktop too old to read one says so, and the tab falls back to its terminal.
 */
const CONVERSATION_AGENTS = new Set(["claude", "codex", "opencode", "mimo", "gemini", "qwen", "copilot", "cursor-agent"]);

/**
 * A terminal tab's body under its header. For Claude Code and Codex: their
 * conversation (default) or the terminal, switched with an icon in the
 * message box; for anything else the terminal.
 */
function AgentTabScreen({ deviceId, tab, header, workspaceRoot }: {
  deviceId: string;
  tab: RelayTab;
  header: (extra: ReactNode) => ReactNode;
  workspaceRoot: string | null;
}) {
  const conversational = CONVERSATION_AGENTS.has((tab.agent_kind ?? "").toLowerCase());
  const [view, setView] = useState<"conversation" | "terminal">(conversational ? "conversation" : "terminal");
  const [unsupported, setUnsupported] = useState(false);
  const showTerminal = useCallback(() => setView("terminal"), []);
  const markUnsupported = useCallback(() => { setUnsupported(true); setView("terminal"); }, []);
  const switchable = conversational && !unsupported;
  const viewSwitch = switchable ? <ViewSwitch value={view} onChange={setView} /> : undefined;
  const terminal = (
    <CompanionTerminalView
      deviceId={deviceId}
      tabId={tab.id}
      onTakenByDesktop={() => {
        if (router.canGoBack()) router.back();
      }}
      renderInput={(api) => (
        <TerminalComposer api={api} tabId={tab.id} workspaceRoot={workspaceRoot} viewSwitch={viewSwitch} />
      )}
    />
  );
  // The terminal / chat switch lives in the message box, so
  // the header keeps its room for the tab's own tools.
  return (
    <>
      {header(null)}
      {switchable && view === "conversation" ? (
        <CliConversationView
          tab={tab}
          onOpenTerminal={showTerminal}
          onUnsupported={markUnsupported}
          workspaceRoot={workspaceRoot}
          viewSwitch={viewSwitch}
        />
      ) : (
        terminal
      )}
    </>
  );
}

/** Records the open tab as the place to resume (renders nothing). */
function RememberResume({ point }: { point: ResumePoint }) {
  const { deviceId, tabId, title, context, desktopName, mark } = point;
  useEffect(() => {
    void rememberResume({ deviceId, tabId, title, context, desktopName, at: Date.now(), ...(mark ? { mark } : {}) });
  }, [deviceId, tabId, title, context, desktopName, mark]);
  return null;
}

function tabKindLabel(kind: string, agentKind: string | null): string {
  if (kind === "agent") return agentKind ? `Agent · ${agentKind}` : "Agent";
  if (kind === "terminal") return agentKind ? `Terminal · ${agentKind}` : "Terminal";
  if (kind === "browser") return "Browser";
  return "Settings";
}

function agentStateLabel(state: string | null): string {
  if (state === "working") return "Working";
  if (state === "review") return "In review";
  return "Idle";
}

/**
 * Live tab: real per-tab host terminal for terminal/agent kinds via the
 * companion bridge (attach/read/screen/input/resize/detach over the existing
 * machine socket), honest unavailable for browser forwarding. No preview
 * data is ever shown and no terminal output is invented: polling is
 * sequential and bounded (200ms, foreground + socket ready + machine online
 * only), offsets stay exact, gaps resync explicitly, and detach releases
 * transport without killing the desktop PTY.
 */
export function MachineTabScreen({
  deviceId,
  tabId,
}: {
  deviceId: string;
  tabId: number;
}) {
  const machine = useMachine();
  const { catalog } = machine;
  const { theme } = useTheme();
  // Every state paints the theme background: without it the screen showed the
  // platform's white behind the status badge and terminal status lines,
  // whose theme-muted text was then faint on white.
  const frame = [styles.frame, { backgroundColor: theme.colors.background }];
  // The strip's "+" bumps this to open the workspace menu's new-tab choices.
  const [newTabSignal, setNewTabSignal] = useState(0);
  // Looking at the tab here counts as seeing it on the desktop.
  useTabSeen(tabId);

  if (catalog.status === "loading") {
    return (
      <View style={frame}>
        <MachineHeader title="Tab" onBack={() => router.back()} backLabel="Back" online={null} />
        <LoadingState label="Loading this tab…" />
      </View>
    );
  }
  if (catalog.status === "unavailable") {
    return (
      <View style={frame}>
        <MachineHeader title="Tab" onBack={() => router.back()} backLabel="Back" online={null} />
        <View style={styles.padded}>
          <Feedback message={catalog.reason} />
          <Button label="Retry" size="sm" variant="ghost" onPress={machine.reconnect} />
        </View>
      </View>
    );
  }
  if (!catalog.machine.online) {
    return (
      <View style={frame}>
        <MachineHeader title="Tab" onBack={() => router.back()} backLabel="Back" online={false} />
        <EmptyState
          title="This machine is offline"
          message="Tab detail resumes when the desktop reconnects."
          actionLabel="Retry"
          onAction={machine.reconnect}
        />
      </View>
    );
  }

  const tab = catalog.catalog.tabs.find((entry) => entry.id === tabId) ?? null;
  if (!tab) {
    return (
      <View style={frame}>
        <MachineHeader title="Tab unavailable" onBack={() => router.back()} backLabel="Back" online />
        <EmptyState
          title="Tab not found"
          message="This live tab is not part of that machine's catalog."
          actionLabel="Back to machine"
          onAction={() => router.replace({ pathname: "/machine/[deviceId]", params: { deviceId } })}
        />
      </View>
    );
  }

  const workspaceName = tab.workspace_id
    ? (catalog.catalog.workspaces.find((entry) => workspaceKey(entry) === tab.workspace_id)?.name ??
      tab.workspace_id)
    : null;
  const workspace = tab.workspace_id
    ? (catalog.catalog.workspaces.find((entry) => workspaceKey(entry) === tab.workspace_id) ?? null)
    : null;
  const projectName = workspace
    ? (() => {
        const key = workspaceProjectKey(workspace);
        if (!key) return null;
        return catalog.catalog.projects.find((entry) => projectKey(entry) === key)?.name ?? key;
      })()
    : (tab.project_id
        ? (catalog.catalog.projects.find((entry) => projectKey(entry) === tab.project_id)?.name ??
          tab.project_id)
        : null);

  const title = tab.title || tabKindLabel(tab.kind, tab.agent_kind);
  const subtitle = [projectName, workspaceName].filter(Boolean).join(" · ") || catalog.machine.name;
  const backLabel = workspaceName ? `Back to ${workspaceName}` : "Back";
  // Back, and the strip's "All tabs", go to this workspace's tab list
  // whichever way the person came (a workspace row, Resume, a notification,
  // a team): back to it when it's already behind this screen, else in place
  // of this screen, so its own Back goes where this one would have.
  const toWorkspace = workspace
    ? () => router.dismissTo(machineWorkspacePath(deviceId, workspaceKey(workspace)))
    : () => router.back();
  // Home's "Resume" brings the person back here.
  const remember = (
    <RememberResume
      point={{
        deviceId,
        tabId: tab.id,
        title,
        context: [projectName, workspaceName].filter(Boolean).join(" · "),
        desktopName: catalog.catalog.machine?.name || catalog.machine.name,
        at: Date.now(),
        mark: isDragentTab(tab) ? "dragent" : tab.agent_kind || (tab.kind === "browser" ? "browser" : "terminal"),
      }}
    />
  );
  // The open tab's workspace: its tabs drive the strip and the tools menu.
  // A tab without a workspace (legacy) shows no strip.
  const workspaceTabs = workspace ? tabsForWorkspace(catalog.catalog, workspace) : null;
  const tools = workspace ? (
    <WorkspaceTools deviceId={deviceId} workspace={workspace} tabId={tab.id} tabTitle={title} newTabSignal={newTabSignal} />
  ) : undefined;
  const strip = workspace && workspaceTabs ? (
    <WorkspaceTabStrip
      deviceId={deviceId}
      workspaceName={workspace.name}
      tabs={workspaceTabs}
      activeTabId={tab.id}
      onAllTabs={toWorkspace}
      onNewTab={() => setNewTabSignal((n) => n + 1)}
    />
  ) : null;

  // A browser tab: the real tab on the desktop, shown as a live picture and
  // driven from here (address, back/forward/reload, tap, scroll, typing).
  if (tab.kind === "browser") {
    return (
      <View style={frame}>
        <MachineHeader title={title} subtitle={subtitle} onBack={toWorkspace} backLabel={backLabel} online actions={tools} />
        {strip}
        {remember}
        <BrowserTabView
          tabId={tab.id}
          onTakenByDesktop={() => {
            if (router.canGoBack()) router.back();
          }}
        />
      </View>
    );
  }

  // Real full-height host terminal for terminal/agent kinds. The built-in
  // Dragent agent does its work in the desktop execution runtime, not in the
  // host PTY the companion bridge reads — so a working Dragent tab renders
  // the recent activity feed instead of a blank idle terminal. Terminal CLIs
  // and other agents keep the PTY unchanged. Status lives in the tab strip
  // above, so no status card is rendered here.
  if (tab.kind === "terminal" || tab.kind === "agent") {
    if (isDragentTab(tab)) {
      const activity = dragentActivity(tab);
      return (
        <View style={frame}>
          <MachineHeader title={title} subtitle={subtitle} onBack={toWorkspace} backLabel={backLabel} online actions={tools} />
          {strip}
        {remember}
          {/* The conversation itself, with a message box. An older desktop
              that can't stream it keeps the earlier view (activity or PTY). */}
          <DragentAgentView
            tabId={tab.id}
            workspaceRoot={workspace?.path ?? null}
            fallback={
              activity.length > 0 ? (
                <ScrollView
                  style={styles.feed}
                  contentContainerStyle={styles.feedContent}
                  accessibilityLabel="Dragent activity"
                  contentInsetAdjustmentBehavior="never"
                  automaticallyAdjustContentInsets={false}
                >
                  {dragentActivityBlocks(activity).map((block) => (
                    <AgentBlock key={`dragent-activity:${tab.id}:${block.id}`} block={block} />
                  ))}
                </ScrollView>
              ) : (
                <CompanionTerminalView
                  deviceId={deviceId}
                  tabId={tab.id}
                  onTakenByDesktop={() => {
                    if (router.canGoBack()) router.back();
                  }}
                  renderInput={(api) => (
                    <TerminalComposer api={api} tabId={tab.id} workspaceRoot={workspace?.path ?? null} />
                  )}
                />
              )
            }
          />
        </View>
      );
    }
    return (
      <View style={frame}>
        {remember}
        <AgentTabScreen
          deviceId={deviceId}
          tab={tab}
          workspaceRoot={workspace?.path ?? null}
          header={(extra) => (
            <>
              <MachineHeader
                title={title}
                subtitle={subtitle}
                onBack={toWorkspace}
                backLabel={backLabel}
                online
                actions={extra ? <View style={styles.actions}>{extra}{tools}</View> : tools}
              />
              {strip}
            </>
          )}
        />
      </View>
    );
  }

  return (
    <View style={frame}>
      <MachineHeader
        title={title}
        subtitle={subtitle}
        onBack={toWorkspace}
        backLabel={backLabel}
        online
        actions={tools}
      />
      {strip}
        {remember}
      <ScrollScreen maxWidth={640} contentStyle={styles.content}>
        <Section
          title={tabKindLabel(tab.kind, tab.agent_kind)}
          description={[projectName, workspaceName].filter(Boolean).join(" · ") || "Live tab"}
          actions={
            <StatusBadge
              label={agentStateLabel(tab.agent_state)}
              tone={tab.agent_state === "working" ? "accent" : tab.agent_state === "review" ? "warning" : "neutral"}
            />
          }
        >
          <View style={styles.facts}>
            <AppText variant="caption" tone="textMuted">Tab id {tab.id}</AppText>
            <AppText variant="caption" tone="textMuted">
              {tab.active ? "Active" : "Inactive"} · {tab.visible ? "Visible" : "Hidden"}
            </AppText>
            <AppText variant="caption" tone="textMuted">
              Workspace {workspaceName ?? "not linked"} · Project {projectName ?? "not linked"}
            </AppText>
          </View>
        </Section>
      </ScrollScreen>
    </View>
  );
}

const styles = StyleSheet.create({
  frame: { flex: 1, minHeight: 0 },
  content: { gap: spacing.xl },
  padded: { gap: 12, padding: 16 },
  facts: { gap: spacing.xs },
  feed: { flex: 1, minHeight: 0 },
  actions: { flexDirection: "row", alignItems: "center", gap: 4 },
  feedContent: { gap: spacing.md, padding: 12, paddingBottom: spacing.xl },
});
