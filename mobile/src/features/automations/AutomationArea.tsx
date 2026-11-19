import { useMemo } from "react";
import { FlatList, SectionList, StyleSheet, View } from "react-native";
import { router, useLocalSearchParams } from "expo-router";
import { AppText, Control, EmptyState, Icon, StatusBadge } from "../../components";
import { TabStrip } from "../../components/TabStrip";
import { Screen } from "../../layout";
import { useTheme } from "../../theme";
import { spacing } from "../../theme/tokens";
import { usePreview } from "../../preview/PreviewContext";
import { selectAutomation } from "../../preview/machineSelectors";
import type { PreviewAutomation, PreviewAutomationRun } from "../../preview/machineTypes";
import {
  AUTOMATION_RUN_STATUS,
  automationScopeOptions,
  formatAutomationRunAt,
  parseAutomationScope,
  type AutomationScope,
} from "./automationScope";

interface MineRow {
  key: string;
  automation: PreviewAutomation;
  projectName: string;
  lastAt: number | null;
  lastWhen: string | null;
  runCount: number;
}

interface InboxRow {
  key: string;
  run: PreviewAutomationRun;
  automation: PreviewAutomation;
  projectName: string;
  when: string;
}

function MineRowView({ row, onOpen }: { row: MineRow; onOpen: (automation: PreviewAutomation) => void }) {
  const { theme } = useTheme();
  const { automation } = row;
  const status = automation.enabled ? "Active" : "Paused";
  return (
    <Control
      accessibilityRole="button"
      accessibilityLabel={`${automation.name}, ${status}, ${row.projectName}, schedule ${automation.schedule}${row.lastWhen ? `, last run ${row.lastWhen}` : ", no runs yet"}`}
      onPress={() => onOpen(automation)}
      style={({ pressed, hovered }) => [
        styles.row,
        {
          borderColor: theme.colors.border,
          backgroundColor: pressed || hovered ? theme.colors.surfaceRaised : theme.colors.background,
        },
      ]}
    >
      <View style={[styles.icon, { backgroundColor: theme.colors.surfaceRaised, borderColor: theme.colors.border }]}>
        <Icon name="agent" size={18} tone="text" />
      </View>
      <View style={styles.copy}>
        <AppText style={styles.name} numberOfLines={2}>{automation.name}</AppText>
        <AppText variant="caption" tone="textMuted" numberOfLines={2}>
          {`${row.projectName} · ${automation.schedule}`}
        </AppText>
        <AppText variant="caption" tone="textSubtle" numberOfLines={2}>
          {row.lastWhen
            ? `${status} · Last run ${row.lastWhen} · ${row.runCount === 1 ? "1 run" : `${row.runCount} runs`}`
            : `${status} · No runs yet`}
        </AppText>
      </View>
      <View style={styles.trailing}>
        <StatusBadge label={status} tone={automation.enabled ? "success" : "neutral"} />
        <Icon name="chevron-right" size={16} />
      </View>
    </Control>
  );
}

function InboxRowView({ row, onOpen }: { row: InboxRow; onOpen: (automation: PreviewAutomation) => void }) {
  const { theme } = useTheme();
  const status = AUTOMATION_RUN_STATUS[row.run.status] ?? { label: row.run.status, tone: "neutral" as const };
  return (
    <Control
      accessibilityRole="button"
      accessibilityLabel={`${row.automation.name}, ${status.label}, ${row.projectName}, ${row.when}`}
      onPress={() => onOpen(row.automation)}
      style={({ pressed, hovered }) => [
        styles.row,
        {
          borderColor: theme.colors.border,
          backgroundColor: pressed || hovered ? theme.colors.surfaceRaised : theme.colors.background,
        },
      ]}
    >
      <View style={[styles.icon, { backgroundColor: theme.colors.surfaceRaised, borderColor: theme.colors.border }]}>
        <Icon name="agent" size={18} tone="text" />
      </View>
      <View style={styles.copy}>
        <AppText style={styles.name} numberOfLines={2}>{row.automation.name}</AppText>
        <AppText variant="caption" tone="textMuted" numberOfLines={2}>
          {`${row.projectName} · ${row.when}`}
        </AppText>
        <AppText variant="caption" tone="textSubtle" numberOfLines={2}>{row.run.output}</AppText>
      </View>
      <View style={styles.trailing}>
        <StatusBadge label={status.label} tone={status.tone} />
        <Icon name="chevron-right" size={16} />
      </View>
    </Control>
  );
}

/**
 * Desktop-style automation list: Mine (this machine's sample automations,
 * grouped Up next / Paused), Team (honest empty state — the sample schema
 * has no teammate owner data), and Inbox (this machine's sample run
 * results). Schedules stay read-only; no creation form in this pass.
 */
export function AutomationArea({ desktopId }: { desktopId: string }) {
  const preview = usePreview();
  const { automationScope: rawScope } = useLocalSearchParams<{ automationScope?: string | string[] }>();
  const scope: AutomationScope = parseAutomationScope(Array.isArray(rawScope) ? rawScope[0] : rawScope);

  const desktop = preview.desktops.find((entry) => entry.id === desktopId) ?? null;

  const mine = useMemo<MineRow[]>(() => {
    if (!desktop) return [];
    return preview.automations
      .filter((automation) => selectAutomation(preview, desktop.id, automation.id) !== null)
      .map((automation) => {
        const project = preview.projects.find((entry) => entry.id === automation.projectId);
        const runs = preview.automationRuns.filter((run) => run.automationId === automation.id);
        const last = runs.reduce<(typeof runs)[number] | null>(
          (latest, run) => (!latest || run.at > latest.at ? run : latest),
          null,
        );
        return {
          key: automation.id,
          automation,
          projectName: project?.name ?? automation.projectId,
          lastAt: last?.at ?? null,
          lastWhen: last ? formatAutomationRunAt(automation.timeZone, last.at) : null,
          runCount: runs.length,
        };
      });
  }, [preview, desktop]);

  const inbox = useMemo<InboxRow[]>(() => {
    if (!desktop) return [];
    return preview.automationRuns
      .map((run) => {
        const automation = selectAutomation(preview, desktop.id, run.automationId);
        if (!automation) return null;
        const project = preview.projects.find((entry) => entry.id === automation.projectId);
        return {
          key: run.id,
          run,
          automation,
          projectName: project?.name ?? automation.projectId,
          when: formatAutomationRunAt(automation.timeZone, run.at),
        };
      })
      .filter((row): row is InboxRow => row !== null)
      .sort((a, b) => b.run.at - a.run.at);
  }, [preview, desktop]);

  const sections = useMemo(() => {
    const upNext = mine.filter((row) => row.automation.enabled);
    const paused = mine.filter((row) => !row.automation.enabled);
    return [
      ...(upNext.length > 0 ? [{ title: "Up next", data: upNext }] : []),
      ...(paused.length > 0 ? [{ title: "Paused", data: paused }] : []),
    ];
  }, [mine]);

  const openAutomation = (automation: PreviewAutomation) =>
    router.push({
      pathname: "/preview/desktop/[desktopId]/automation/[automationId]",
      params: { desktopId, automationId: automation.id, automationScope: scope },
    });

  if (!desktop) {
    return (
      <Screen maxWidth={1040} contentStyle={styles.content}>
        <EmptyState
          title="Desktop not found"
          message="This sample desktop is unavailable. It may have been reset."
          actionLabel="Back to preview"
          onAction={() => router.replace("/preview")}
        />
      </Screen>
    );
  }

  return (
    <Screen maxWidth={1040} contentStyle={styles.content}>
      <AppText variant="caption" tone="textMuted">
        {scope === "inbox"
          ? `Sample run results on ${desktop.name}. Nothing is scheduled or executed.`
          : `Sample automations on ${desktop.name}. Schedules are read-only in this preview.`}
      </AppText>
      <TabStrip
        label="Automation scope"
        options={automationScopeOptions(mine.length, inbox.length)}
        value={scope}
        onChange={(next) => router.setParams({ automationScope: next })}
      />
      {scope === "team" ? (
        <EmptyState
          title="No team automation samples"
          message="Teammate automations aren't part of this sample set. Only this machine's sample automations appear here."
        />
      ) : scope === "inbox" ? (
        <FlatList
          style={styles.list}
          data={inbox}
          keyExtractor={(item) => item.key}
          renderItem={({ item }) => <InboxRowView row={item} onOpen={openAutomation} />}
          keyboardShouldPersistTaps="handled"
          contentContainerStyle={[styles.listContent, inbox.length === 0 ? styles.emptyList : undefined]}
          ListEmptyComponent={
            <EmptyState
              title="No results yet"
              message="When a sample run finishes it shows up here."
            />
          }
        />
      ) : (
        <SectionList
          style={styles.list}
          sections={sections}
          keyExtractor={(item) => item.key}
          renderItem={({ item }) => <MineRowView row={item} onOpen={openAutomation} />}
          renderSectionHeader={({ section: { title } }) => (
            <AppText variant="caption" tone="textMuted" style={styles.sectionTitle}>{title}</AppText>
          )}
          keyboardShouldPersistTaps="handled"
          contentContainerStyle={[styles.listContent, sections.length === 0 ? styles.emptyList : undefined]}
          ListEmptyComponent={
            <EmptyState
              title="No automations"
              message={`${desktop.name} has no sample automations. Automation samples live on the macOS desktop in this preview.`}
            />
          }
        />
      )}
    </Screen>
  );
}

const styles = StyleSheet.create({
  content: { gap: spacing.md },
  list: { flex: 1, minHeight: 0 },
  listContent: { paddingBottom: spacing.xl },
  emptyList: { flexGrow: 1 },
  sectionTitle: { paddingTop: spacing.sm },
  row: {
    minHeight: 64,
    borderBottomWidth: 1,
    flexDirection: "row",
    alignItems: "center",
    gap: spacing.md,
    paddingVertical: spacing.sm,
    paddingHorizontal: spacing.xs,
  },
  icon: {
    width: 36,
    height: 36,
    borderRadius: 10,
    borderWidth: 1,
    alignItems: "center",
    justifyContent: "center",
    flexShrink: 0,
  },
  copy: { flex: 1, minWidth: 0, gap: 2 },
  name: { fontWeight: "600" },
  trailing: { flexDirection: "row", alignItems: "center", gap: spacing.md, flexShrink: 0 },
});
