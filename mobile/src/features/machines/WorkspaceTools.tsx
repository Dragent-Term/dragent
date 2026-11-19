import { useMemo, type ReactNode } from "react";
import { StyleSheet, View } from "react-native";
import { router } from "expo-router";
import { AgentMark, AgentStateDot, AppText, BrowserMark, IconButton, TerminalMark, agentStateLabel } from "../../components";
import { Control } from "../../components/Control";
import { Icon } from "../../components/Icon";
import { TabStrip } from "../../components/TabStrip";
import { useTheme } from "../../theme";
import { tabNeed } from "../../machines/attention";
import { othersNeedingYou } from "../../machines/workspaceTabs";
import type { RelayTab, RelayWorkspace } from "../../transport/relayCatalog";
import { dragentStatusLine, isDragentTab } from "../../transport/relayCatalog";
import { machineTabPath } from "../../machines/routes";
import { buildTabStripItems } from "../../machines/tabStrip";
import { WorkspaceMenu } from "../workspaceMenu/WorkspaceMenu";

function liveTabMark(kind: RelayTab["kind"], agent: string | null): ReactNode {
  const name = (agent ?? "").trim().toLowerCase();
  if (kind === "agent" || (kind === "terminal" && name !== "")) {
    if (name !== "") return <AgentMark agent={name} size={13} />;
  }
  if (kind === "terminal" || kind === "agent") return <TerminalMark size={13} />;
  if (kind === "browser") return <BrowserMark size={13} />;
  return undefined;
}

/**
 * "All tabs": the workspace's tab list, with how many tabs it has and a dot
 * when another of them waits for the person.
 */
function AllTabsButton({ count, waiting, onPress }: { count: number; waiting: number; onPress: () => void }) {
  const { theme } = useTheme();
  const label = waiting > 0
    ? `All ${count} tabs, ${waiting} other ${waiting === 1 ? "needs" : "need"} you`
    : `All ${count} tabs`;
  return (
    <Control
      accessibilityRole="button"
      accessibilityLabel={label}
      onPress={onPress}
      style={({ pressed, hovered }) => [
        styles.allTabs,
        { backgroundColor: pressed || hovered ? theme.colors.surfaceRaised : "transparent" },
      ]}
    >
      <Icon name="layers" size={17} />
      <AppText variant="caption" tone="textMuted" style={styles.count}>{count}</AppText>
      {waiting > 0 ? (
        <View style={styles.badgeDot}>
          <AgentStateDot state="waiting" />
        </View>
      ) : null}
    </Control>
  );
}

/**
 * Horizontally scrollable strip of every tab in the open tab's workspace.
 * Each option carries its status dot and agent mark; tapping one switches
 * to it with `replace` so Back still returns to the workspace. A working
 * Dragent tab shows the desktop's status line (`Dragent · <work> · <step>`)
 * instead of its bare title, so the strip agrees with the tab screen.
 *
 * At its end, kept in place however far the tabs scroll: "All tabs" (the
 * workspace's tab list) and "+" (a new tab). A tab that needs the person
 * and is scrolled out of sight shows a dot at that edge.
 */
export function WorkspaceTabStrip({
  deviceId,
  workspaceName,
  tabs,
  activeTabId,
  onAllTabs,
  onNewTab,
}: {
  deviceId: string;
  workspaceName: string;
  tabs: readonly RelayTab[];
  activeTabId: number;
  onAllTabs?: () => void;
  onNewTab?: () => void;
}) {
  const items = useMemo(() => buildTabStripItems(tabs), [tabs]);
  const byId = useMemo(() => new Map(tabs.map((tab) => [tab.id, tab])), [tabs]);
  const attention = useMemo(
    () => new Set(tabs.filter((tab) => tab.id !== activeTabId && tabNeed(tab) !== null).map((tab) => String(tab.id))),
    [tabs, activeTabId],
  );
  const trailing = onAllTabs || onNewTab ? (
    <>
      {onAllTabs ? (
        <AllTabsButton count={tabs.length} waiting={othersNeedingYou(tabs, activeTabId)} onPress={onAllTabs} />
      ) : null}
      {onNewTab ? <IconButton name="plus" label="New tab" size="sm" onPress={onNewTab} /> : null}
    </>
  ) : undefined;
  return (
    <TabStrip
      label={`${workspaceName} tabs`}
      trailing={trailing}
      attention={attention}
      options={items.map((item) => {
        const live = item.dot !== "idle" ? `, ${agentStateLabel(item.dot)}` : "";
        const tab = byId.get(item.tabId);
        // Bare `Dragent` carries no more than the kind label, so only a
        // status line with work/step context replaces the strip label.
        const line = tab && isDragentTab(tab) ? dragentStatusLine(tab) : null;
        const label = line !== null && line !== "Dragent" ? line : item.label;
        const hint =
          label !== item.label
            ? `${label}, ${item.kindLabel}${live}`
            : item.title
              ? `${item.title}, ${item.kindLabel}${live}`
              : `${item.kindLabel}${live}`;
        return {
          id: item.id,
          label,
          hint,
          leading: (
            <View style={styles.tabLeading}>
              <AgentStateDot state={item.dot} />
              {liveTabMark(item.kind, item.agent)}
            </View>
          ),
        };
      })}
      value={String(activeTabId)}
      onChange={(next) => {
        const target = items.find((item) => item.id === next);
        if (!target || target.tabId === activeTabId) return;
        router.replace(machineTabPath(deviceId, target.tabId));
      }}
    />
  );
}

/**
 * The ⋯ button in a tab's header: the same workspace menu as the workspace
 * screen's (Files, Workspace changes, New tab, Close). A tab opened from
 * here replaces the current one, so Back still returns to the workspace.
 */
export function WorkspaceTools({
  deviceId,
  workspace,
  tabTitle,
  newTabSignal = 0,
}: {
  deviceId: string;
  workspace: RelayWorkspace;
  tabId: number;
  tabTitle: string;
  /** Bumped by the strip's "+" to open the new-tab choices. */
  newTabSignal?: number;
}) {
  return (
    <WorkspaceMenu
      deviceId={deviceId}
      workspace={workspace}
      trigger="dots"
      backTitle={tabTitle}
      replaceOnNewTab
      newTabSignal={newTabSignal}
    />
  );
}

const styles = StyleSheet.create({
  tabLeading: { flexDirection: "row", alignItems: "center", gap: 4, flexShrink: 0 },
  allTabs: { minHeight: 36, minWidth: 44, paddingHorizontal: 8, borderRadius: 8, flexDirection: "row", alignItems: "center", justifyContent: "center", gap: 4 },
  count: { fontVariant: ["tabular-nums"] },
  badgeDot: { position: "absolute", top: 4, right: 4 },
});
