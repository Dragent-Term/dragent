import { useMemo, useState } from "react";
import { FlatList, StyleSheet } from "react-native";
import { router } from "expo-router";
import { AppText, EmptyState, TabStrip } from "../../components";
import { Screen } from "../../layout";
import { spacing } from "../../theme/tokens";
import { usePreview } from "../../preview/PreviewContext";
import type { PreviewCoordination } from "../../preview/machineTypes";
import {
  COORDINATION_FILTER_OPTIONS,
  COORDINATION_STATUS_META,
  CoordinationListRow,
  matchesCoordinationFilter,
  type CoordinationFilter,
  type CoordinationRowModel,
} from "../../coordination/CoordinationViews";

export const COORDINATION_STATUS = COORDINATION_STATUS_META;

interface CoordinationRow {
  key: string;
  coordination: PreviewCoordination;
  projectName: string;
  doneCount: number;
  totalCount: number;
}

function toSharedRow(row: CoordinationRow): CoordinationRowModel {
  return {
    key: row.key,
    id: row.coordination.id,
    title: row.coordination.title,
    goal: row.coordination.goal,
    meta: `${row.projectName} · ${row.doneCount} of ${row.totalCount} done`,
    doneCount: row.doneCount,
    totalCount: row.totalCount,
    status: row.coordination.status,
  };
}

/**
 * Agent-coordination area for the machine hub: filter (All/Active/Finished)
 * plus a virtualized list of this desktop's coordinations. Choosing a row
 * opens the coordination detail route; the hub owns all chrome and routes.
 */
export function CoordinationArea({ desktopId }: { desktopId: string }) {
  const preview = usePreview();
  const [filter, setFilter] = useState<CoordinationFilter>("all");

  const desktop = preview.desktops.find((entry) => entry.id === desktopId) ?? null;

  const rows = useMemo<CoordinationRow[]>(() => {
    if (!desktop) return [];
    return preview.coordinations
      .filter((coordination) => coordination.desktopId === desktop.id && matchesCoordinationFilter(coordination.status, filter))
      .map((coordination) => {
        const project = preview.projects.find(
          (entry) => entry.id === coordination.projectId && entry.desktopId === desktop.id,
        );
        return {
          key: coordination.id,
          coordination,
          projectName: project?.name ?? "Unknown project",
          doneCount: coordination.plan.filter((task) => task.status === "done").length,
          totalCount: coordination.plan.length,
        };
      });
  }, [preview, desktop, filter]);

  const openCoordination = (coordinationId: string) =>
    router.push({
      pathname: "/preview/desktop/[desktopId]/coordination/[coordinationId]",
      params: { desktopId, coordinationId },
    });

  if (!desktop) {
    return (
      <Screen maxWidth={1040} contentStyle={styles.content}>
        <EmptyState
          title="Machine not found"
          message="This sample machine is no longer available."
        />
      </Screen>
    );
  }

  return (
    <Screen maxWidth={1040} contentStyle={styles.content}>
      <AppText variant="caption" tone="textMuted">
        Sample coordination on {desktop.name}. Plans run locally in this preview.
      </AppText>
      <TabStrip label="Coordination filter" options={COORDINATION_FILTER_OPTIONS} value={filter} onChange={setFilter} />
      <FlatList
        style={styles.list}
        data={rows}
        keyExtractor={(item) => item.key}
        renderItem={({ item }) => <CoordinationListRow row={toSharedRow(item)} onOpen={openCoordination} />}
        keyboardShouldPersistTaps="handled"
        contentContainerStyle={[styles.listContent, rows.length === 0 ? styles.emptyList : undefined]}
        ListEmptyComponent={
          filter === "all" ? (
            <EmptyState title="No coordination yet" message="Sample multi-agent work appears here." />
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
    </Screen>
  );
}

const styles = StyleSheet.create({
  content: { gap: spacing.md },
  list: { flex: 1, minHeight: 0 },
  listContent: { paddingBottom: spacing.xl },
  emptyList: { flexGrow: 1 },
});
