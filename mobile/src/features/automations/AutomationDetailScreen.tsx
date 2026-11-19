import { useMemo, useState } from "react";
import { FlatList, StyleSheet, Switch, View } from "react-native";
import { router, useLocalSearchParams } from "expo-router";
import { AppText, Button, EmptyState, Icon, StatusBadge } from "../../components";
import { TabStrip } from "../../components/TabStrip";
import { Screen, ScrollScreen } from "../../layout";
import { useTheme } from "../../theme";
import { spacing } from "../../theme/tokens";
import { usePreview } from "../../preview/PreviewContext";
import { PreviewHeader } from "../../preview/PreviewHeader";
import { selectAutomation, selectMachineTab } from "../../preview/machineSelectors";
import type { PreviewAutomationRun } from "../../preview/machineTypes";
import {
  AUTOMATION_RUN_STATUS,
  formatAutomationRunAt,
  parseAutomationScope,
} from "./automationScope";

type DetailView = "runs" | "settings";

function SettingRow({ label, value, selectable }: { label: string; value: string; selectable?: boolean }) {
  const { theme } = useTheme();
  return (
    <View style={[styles.setting, { borderColor: theme.colors.border }]}>
      <AppText variant="caption" tone="textMuted">{label}</AppText>
      <AppText selectable={selectable}>{value}</AppText>
    </View>
  );
}

/** Compact desktop-style schedule/paused banner for one automation. */
function StatusBanner({ enabled, schedule, timeZone }: {
  enabled: boolean;
  schedule: string;
  timeZone: string;
}) {
  const { theme } = useTheme();
  return (
    <View
      accessible
      accessibilityLabel={enabled ? `Active, ${schedule}` : "Paused"}
      style={[styles.banner, { borderColor: theme.colors.border, backgroundColor: theme.colors.surfaceRaised }]}
    >
      <Icon name="clock" size={14} tone={enabled ? "success" : "textMuted"} />
      <AppText variant="caption" tone={enabled ? "success" : "textMuted"} style={styles.bannerText}>
        {enabled ? `Active · ${schedule} (${timeZone})` : "Paused · Turn it on to schedule runs again."}
      </AppText>
    </View>
  );
}

function RunRow({ run, when, workspaceLabel, canOpen, onOpen }: {
  run: PreviewAutomationRun;
  when: string;
  workspaceLabel: string;
  canOpen: boolean;
  onOpen: () => void;
}) {
  const status = AUTOMATION_RUN_STATUS[run.status] ?? { label: run.status, tone: "neutral" as const };
  const { theme } = useTheme();
  return (
    <View style={[styles.run, { borderColor: theme.colors.border }]}>
      <View style={styles.runHeader}>
        <StatusBadge label={status.label} tone={status.tone} />
        <AppText variant="caption" tone="textMuted">{when}</AppText>
      </View>
      <AppText variant="caption" selectable style={styles.output}>{run.output}</AppText>
      <AppText variant="caption" tone="textSubtle" numberOfLines={2}>{workspaceLabel}</AppText>
      {canOpen ? (
        <View style={styles.runActions}>
          <Button label="Open workspace" variant="ghost" size="sm" onPress={onOpen} />
        </View>
      ) : null}
    </View>
  );
}

/**
 * One automation: read-only schedule/settings plus its local sample run
 * history. Running records a fixed sample result; enabling toggles local
 * state. No creation or scheduler editing in this preview.
 */
export function AutomationDetailScreen({ desktopId, automationId }: {
  desktopId: string;
  automationId: string;
}) {
  const preview = usePreview();
  const { theme } = useTheme();
  const [view, setView] = useState<DetailView>("runs");
  const { automationScope: rawScope } = useLocalSearchParams<{ automationScope?: string | string[] }>();
  const automationScope = parseAutomationScope(Array.isArray(rawScope) ? rawScope[0] : rawScope);

  const desktop = preview.desktops.find((entry) => entry.id === desktopId) ?? null;
  const automation = selectAutomation(preview, desktopId, automationId);
  const timeZone = automation?.timeZone ?? null;
  const project = automation ? preview.projects.find((entry) => entry.id === automation.projectId) ?? null : null;
  const workspace = automation ? preview.workspaces.find((entry) => entry.id === automation.workspaceId) ?? null : null;
  const agentTab = automation ? selectMachineTab(preview, desktopId, automation.agentTabId) : null;

  const runs = useMemo(() => {
    if (!automation) return [];
    return preview.automationRuns
      .filter((run) => run.automationId === automation.id)
      .slice()
      .sort((a, b) => b.at - a.at)
      .map((run) => ({ key: run.id, run, when: formatAutomationRunAt(timeZone, run.at) }));
  }, [preview, automation, timeZone]);

  if (!desktop || !automation || !project || !workspace) {
    return (
      <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
        <PreviewHeader
          title="Automation unavailable"
          onBack={() => router.replace("/preview")}
          backLabel="Back to preview"
        />
        <Screen maxWidth={1040} contentStyle={styles.content}>
          <EmptyState
            title="Automation not found"
            message="This sample automation is unavailable. It may have been reset."
            actionLabel="Back to preview"
            onAction={() => router.replace("/preview")}
          />
        </Screen>
      </View>
    );
  }

  const backToAutomation = () =>
    router.navigate({
      pathname: "/preview/desktop/[desktopId]",
      params: { desktopId, view: "automation", automationScope },
    });

  const openWorkspace = () => {
    if (!agentTab || agentTab.workspaceId !== automation.workspaceId) return;
    router.push({
      pathname: "/preview/desktop/[desktopId]/project/[projectId]/workspace/[workspaceId]/tab/[tabId]",
      params: {
        desktopId,
        projectId: automation.projectId,
        workspaceId: automation.workspaceId,
        tabId: agentTab.id,
      },
    });
  };

  return (
    <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
      <PreviewHeader
        title={automation.name}
        subtitle={`${project.name} · ${workspace.branch}`}
        onBack={backToAutomation}
        backLabel="Back to automation"
      />
      <StatusBanner enabled={automation.enabled} schedule={automation.schedule} timeZone={automation.timeZone} />
      <TabStrip
        label="Automation detail view"
        options={[
          { id: "runs", label: "Runs", hint: "Sample run results" },
          { id: "settings", label: "Settings", hint: "Read-only schedule and target" },
        ]}
        value={view}
        onChange={setView}
      />
      {view === "runs" ? (
        <Screen maxWidth={1040} contentStyle={styles.content}>
          <View style={styles.section}>
            <View style={styles.sectionHeader}>
              <AppText variant="caption" tone="textMuted">
                Local sample runs in {automation.timeZone}. Run sample records a fixed result; nothing is scheduled or executed.
              </AppText>
              <Button label="Run sample" onPress={() => preview.runAutomation(desktopId, automation.id)} />
            </View>
            <FlatList
              style={styles.list}
              data={runs}
              keyExtractor={(item) => item.key}
              renderItem={({ item }) => (
                <RunRow
                  run={item.run}
                  when={item.when}
                  workspaceLabel={`${workspace.name} · ${workspace.branch}`}
                  canOpen={!!agentTab && agentTab.workspaceId === automation.workspaceId}
                  onOpen={openWorkspace}
                />
              )}
              keyboardShouldPersistTaps="handled"
              contentContainerStyle={[styles.listContent, runs.length === 0 ? styles.emptyList : undefined]}
              ListEmptyComponent={
                <EmptyState
                  title="No runs yet"
                  message="Run sample records a fixed local result for this automation."
                  actionLabel="Run sample"
                  onAction={() => preview.runAutomation(desktopId, automation.id)}
                />
              }
            />
          </View>
        </Screen>
      ) : (
        <ScrollScreen maxWidth={1040} contentStyle={styles.content}>
          <View style={styles.section}>
            <View
              style={[styles.toggle, { borderColor: theme.colors.border }]}
            >
              <View style={styles.toggleCopy}>
                <AppText style={styles.toggleTitle}>{automation.enabled ? "Enabled" : "Disabled"}</AppText>
                <AppText variant="caption" tone="textMuted">Local preview state only</AppText>
              </View>
              <Switch
                value={automation.enabled}
                onValueChange={() => { preview.toggleAutomation(desktopId, automation.id); }}
                trackColor={{ false: theme.colors.border, true: theme.colors.accent }}
                thumbColor={automation.enabled ? theme.colors.onAccent : theme.colors.textSubtle}
                ios_backgroundColor={theme.colors.surfaceRaised}
                accessibilityLabel={`Automation ${automation.enabled ? "enabled" : "disabled"}`}
                accessibilityHint={automation.enabled ? "Disables this sample automation." : "Enables this sample automation."}
              />
            </View>
            <SettingRow label="Agent" value={agentTab?.title ?? automation.agentTabId} />
            <SettingRow label="Schedule" value={automation.schedule} />
            <SettingRow label="Time zone" value={automation.timeZone} />
            <SettingRow label="Workspace" value={`${workspace.name} · ${workspace.branch} · pinned`} />
            <SettingRow label="Prompt" value={automation.prompt} selectable />
          </View>
        </ScrollScreen>
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  frame: { flex: 1 },
  content: { gap: spacing.md },
  section: { gap: spacing.md, flex: 1, minHeight: 0 },
  sectionHeader: { gap: spacing.sm },
  list: { flex: 1, minHeight: 0 },
  listContent: { gap: spacing.md, paddingBottom: spacing.xl },
  emptyList: { flexGrow: 1 },
  banner: {
    flexDirection: "row",
    alignItems: "center",
    gap: spacing.sm,
    borderWidth: 1,
    borderRadius: 10,
    borderCurve: "continuous",
    marginHorizontal: spacing.md,
    marginTop: spacing.sm,
    paddingVertical: spacing.sm,
    paddingHorizontal: spacing.md,
  },
  bannerText: { flex: 1, minWidth: 0 },
  run: {
    borderWidth: 1,
    borderRadius: 10,
    borderCurve: "continuous",
    gap: spacing.sm,
    padding: spacing.md,
    minWidth: 0,
  },
  runHeader: { flexDirection: "row", flexWrap: "wrap", alignItems: "center", gap: spacing.sm },
  output: { fontWeight: "400" },
  runActions: { flexDirection: "row", flexWrap: "wrap", gap: spacing.sm },
  toggle: {
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
  toggleCopy: { flex: 1, minWidth: 0, gap: 2 },
  toggleTitle: { fontWeight: "600" },
  setting: {
    borderBottomWidth: 1,
    gap: 2,
    paddingVertical: spacing.sm,
    paddingHorizontal: spacing.xs,
    minWidth: 0,
  },
});
