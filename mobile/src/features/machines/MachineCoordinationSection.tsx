import { useMemo, useState } from "react";
import { FlatList, StyleSheet, View } from "react-native";
import { router } from "expo-router";
import { AppText, Button, EmptyState, Icon, LoadingState, TabStrip } from "../../components";
import { useResponsiveLayout } from "../../layout/useResponsiveLayout";
import { spacing } from "../../theme/tokens";
import { Feedback } from "../shared/Feedback";
import { useLiveCoordinationList } from "../../coordination/useLiveCoordination";
import { machineCoordinationDetailPath, machineNewCoordinationPath } from "../../machines/routes";
import { useMachine } from "../../machines/MachineContext";
import { projectLabelForPath } from "../../machines/machineCatalog";
import {
  COORDINATION_FILTER_OPTIONS,
  CoordinationListRow,
  CoordinationTruncatedNotice,
  matchesCoordinationFilter,
  type CoordinationFilter,
} from "../../coordination/CoordinationViews";

/**
 * Live agent-coordination section for the machine hub. Readonly: the shared
 * All/Active/Finished filter narrows the live list client-side, rows open
 * the live detail route. Ineligible moments (offline, ended session,
 * reconnecting, background) show a meaningful hold with retry or a new
 * session — never stale output, never sample content, never a false empty.
 */
export function MachineCoordinationSection({ deviceId }: { deviceId: string }) {
  const machine = useMachine();
  const layout = useResponsiveLayout();
  const live = useLiveCoordinationList();
  const [filter, setFilter] = useState<CoordinationFilter>("all");

  const items = useMemo(() => {
    if (live.state.status !== "live") return null;
    return [...live.state.result.items]
      .filter((item) => matchesCoordinationFilter(item.status, filter))
      .sort((a, b) => b.updated_at - a.updated_at);
  }, [live.state, filter]);

  if (live.state.status === "loading") {
    return <LoadingState label="Loading live coordination…" />;
  }

  if (live.state.status === "unavailable" && items === null) {
    const ended = live.state.action === "reconnect";
    return (
      <View style={styles.padded}>
        <Feedback message={live.state.reason} />
        <Button
          label={ended ? "Start new session" : "Retry"}
          size="sm"
          variant="ghost"
          onPress={ended ? live.reconnect : live.retry}
        />
      </View>
    );
  }

  if (items === null) {
    return <LoadingState label="Loading live coordination…" />;
  }

  const truncated = live.state.status === "live" && live.state.result.truncated;
  const all = live.state.status === "live" ? live.state.result.items : [];
  const active = all.filter((item) => item.status === "planning" || item.status === "running").length;
  const waiting = all.filter((item) => item.needs_user).length;
  const counts = [`${active} active`, ...(waiting > 0 ? [`${waiting} need${waiting === 1 ? "s" : ""} you`] : [])].join(" · ");
  const gutter = { paddingHorizontal: layout.gutter };
  return (
    <View style={styles.frame}>
      <View style={[styles.toolbar, gutter]}>
        <View style={styles.grow}>
          <AppText style={styles.heading}>Agent coordination</AppText>
          <AppText variant="caption" tone="textMuted" numberOfLines={1}>{counts}</AppText>
        </View>
        <Button
          label="New"
          size="sm"
          leading={<Icon name="plus" size={16} />}
          accessibilityLabel="New coordination"
          onPress={() => router.push(machineNewCoordinationPath(deviceId))}
        />
      </View>
      <TabStrip
        label="Coordination filter"
        options={COORDINATION_FILTER_OPTIONS}
        value={filter}
        onChange={setFilter}
      />
      {truncated ? (
        <View style={gutter}>
          <CoordinationTruncatedNotice />
        </View>
      ) : null}
      <FlatList
        data={items}
        keyExtractor={(item) => item.id}
        style={styles.fill}
        contentContainerStyle={[styles.list, gutter, items.length === 0 ? styles.emptyList : undefined]}
        ItemSeparatorComponent={Separator}
        renderItem={({ item }) => (
          <CoordinationListRow
            row={{
              key: item.id,
              id: item.id,
              title: item.title,
              goal: projectLabelForPath(machine.catalog.status === "live" ? machine.catalog.catalog : null, item.project_path),
              meta: [
                ...(item.running_count > 0 ? [`${item.running_count} running`] : []),
                ...(item.blocked_count > 0 ? [`${item.blocked_count} blocked`] : []),
                `updated ${ago(item.updated_at)}`,
              ].join(" · "),
              doneCount: item.done_count,
              totalCount: item.task_count,
              status: item.status,
              needsYou: item.needs_user,
            }}
            onOpen={(teamId) => {
              router.push(machineCoordinationDetailPath(deviceId, teamId));
            }}
          />
        )}
        ListEmptyComponent={
          filter === "all" ? (
            <EmptyState
              title="No coordination yet"
              message="Start one here: a lead agent plans your goal and hands the parts to worker agents on the desktop."
              actionLabel="New coordination"
              onAction={() => router.push(machineNewCoordinationPath(deviceId))}
            />
          ) : (
            <EmptyState
              title={`No ${filter === "active" ? "active" : "finished"} coordination`}
              message="Try another filter."
              actionLabel="Show all"
              onAction={() => setFilter("all")}
            />
          )
        }
      />
    </View>
  );
}

function Separator() {
  return <View style={styles.separator} />;
}

/** "just now", "5 min ago", "3 h ago", "2 d ago". */
export function ago(ms: number): string {
  const seconds = Math.max(0, (Date.now() - ms) / 1000);
  if (seconds < 60) return "just now";
  if (seconds < 3600) return `${Math.floor(seconds / 60)} min ago`;
  if (seconds < 86_400) return `${Math.floor(seconds / 3600)} h ago`;
  return `${Math.floor(seconds / 86_400)} d ago`;
}

const styles = StyleSheet.create({
  frame: { flex: 1, minHeight: 0 },
  fill: { flex: 1, minHeight: 0 },
  list: { paddingTop: spacing.md, paddingBottom: spacing.xl },
  emptyList: { flexGrow: 1 },
  separator: { height: spacing.sm },
  padded: { gap: 12, padding: 16 },
  toolbar: { flexDirection: "row", alignItems: "center", gap: spacing.md, paddingVertical: spacing.md },
  heading: { fontWeight: "600" },
  grow: { flex: 1, minWidth: 0 },
});
