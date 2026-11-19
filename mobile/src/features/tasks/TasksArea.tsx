import { useEffect, useMemo, useState } from "react";
import { SectionList, StyleSheet, View, useWindowDimensions } from "react-native";
import {
  AppText,
  Avatar,
  Button,
  Control,
  EmptyState,
  Icon,
  IconButton,
  LoadingState,
  SearchField,
  SegmentedControl,
  SelectPopover,
  StatusBadge,
} from "../../components";
import { Screen } from "../../layout";
import { useTheme } from "../../theme";
import { spacing } from "../../theme/tokens";
import { usePreview } from "../../preview/PreviewContext";
import { usePreviewRouteParams } from "../../preview/usePreviewRouteParams";
import { TaskDetail } from "./TaskDetail";
import {
  TASK_STATUS,
  TASK_STATUS_FILTERS,
  filterPreviewTasks,
  groupTasksByProject,
  parseTaskStatusFilter,
  taskProjectName,
  taskReference,
  timeAgo,
  type PreviewTask,
  type TaskSection,
  type TaskStatusFilter,
} from "./taskSamples";

/** One quiet task row: title, project ref, author avatar, time, status. */
function TaskListRow({
  task,
  project,
  selected,
  showChevron,
  onOpen,
}: {
  task: PreviewTask;
  project: string;
  selected: boolean;
  showChevron: boolean;
  onOpen: () => void;
}) {
  const { theme } = useTheme();
  const status = TASK_STATUS[task.status];
  return (
    <Control
      accessibilityRole="button"
      accessibilityLabel={`${task.title}, ${project} ${taskReference(task)}, ${status.label}`}
      accessibilityState={{ selected }}
      onPress={onOpen}
      style={({ pressed, hovered }) => [
        styles.row,
        {
          borderColor: theme.colors.border,
          backgroundColor: selected
            ? theme.colors.surfaceRaised
            : pressed || hovered
              ? theme.colors.surface
              : undefined,
          borderLeftColor: selected ? theme.colors.accent : "transparent",
        },
      ]}
    >
      <Icon name="tasks" size={16} tone={task.status === "open" ? "accent" : "textMuted"} />
      <View style={styles.copy}>
        <AppText style={styles.name} numberOfLines={2}>
          {task.title}
        </AppText>
        <AppText variant="caption" tone="textMuted" numberOfLines={1}>
          {`${project} · ${taskReference(task)}`}
        </AppText>
        <View style={styles.metaRow}>
          <Avatar name={task.author} size={16} />
          <AppText variant="caption" tone="textMuted" numberOfLines={1} style={styles.metaAuthor}>
            {task.author}
          </AppText>
          <AppText variant="caption" tone="textSubtle" numberOfLines={1}>
            {timeAgo(task.updatedAt)}
          </AppText>
        </View>
      </View>
      <View style={styles.trailing}>
        <StatusBadge label={status.label} tone={status.tone} />
        {showChevron ? <Icon name="chevron-right" size={16} /> : null}
      </View>
    </Control>
  );
}

/**
 * Machine-wide task inbox, mirroring desktop inbox grouping: sample tasks
 * grouped by project with project + status (open/closed) filters and search.
 * Narrow screens open the task detail in place; wide screens (~1050px+)
 * split inbox and detail with the same detail renderer.
 *
 * The desktop id resolves from the optional prop first, then the preview
 * route, so the machine hub can render `<TasksArea />` with no wiring.
 */
export function TasksArea({ desktopId: explicitDesktopId }: { desktopId?: string } = {}) {
  const { theme } = useTheme();
  const preview = usePreview();
  const { width } = useWindowDimensions();
  const { desktopId: routeDesktopId } = usePreviewRouteParams();
  const desktopId = explicitDesktopId ? explicitDesktopId : routeDesktopId;
  const wide = width >= 1050;
  const compact = width < 480;

  const [projectId, setProjectId] = useState("");
  const [status, setStatus] = useState<TaskStatusFilter>("open");
  const [query, setQuery] = useState("");
  const [projectOpen, setProjectOpen] = useState(false);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [refreshNote, setRefreshNote] = useState<string | null>(null);

  useEffect(() => {
    setProjectId("");
    setSelectedId(null);
    setProjectOpen(false);
  }, [desktopId]);

  const desktop = preview.desktops.find((entry) => entry.id === desktopId) ?? null;
  const projects = useMemo(
    () => preview.projects.filter((project) => project.desktopId === desktopId),
    [preview, desktopId],
  );
  const total = useMemo(
    () => filterPreviewTasks(preview, desktopId, { status: "all" }).length,
    [preview, desktopId],
  );
  const openCount = useMemo(
    () => filterPreviewTasks(preview, desktopId, { status: "open" }).length,
    [preview, desktopId],
  );
  const closedCount = useMemo(
    () => filterPreviewTasks(preview, desktopId, { status: "closed" }).length,
    [preview, desktopId],
  );
  const allVisible = useMemo(
    () =>
      filterPreviewTasks(preview, desktopId, {
        projectId: projectId || undefined,
        status,
        query,
      }),
    [preview, desktopId, projectId, status, query],
  );
  const sections = useMemo(
    () => groupTasksByProject(preview, allVisible),
    [preview, allVisible],
  );
  const selected = allVisible.find((task) => task.id === selectedId) ?? allVisible[0] ?? null;
  const showingDetail = !wide && selectedId !== null && selected !== null;

  const refresh = () => {
    setRefreshing(true);
    setRefreshNote(null);
    // Local preview has no provider fetch; re-run selectors on next frame.
    requestAnimationFrame(() => {
      setRefreshing(false);
      setRefreshNote("Preview refreshed locally. No provider was contacted.");
    });
  };

  if (!desktop) {
    return (
      <Screen maxWidth={1040}>
        <EmptyState
          title="Machine not found"
          message="This sample machine is no longer available."
        />
      </Screen>
    );
  }

  const resetFilters = () => {
    setProjectId("");
    setStatus("open");
    setQuery("");
    setProjectOpen(false);
  };

  const openTask = (id: string) => {
    setSelectedId(id);
  };

  const projectOptions = [
    { id: "", label: "All projects" },
    ...projects.map((project) => ({ id: project.id, label: project.name })),
  ];

  const statusOptions = TASK_STATUS_FILTERS.map((option) => {
    const count = option.id === "open" ? openCount : option.id === "closed" ? closedCount : total;
    return {
      id: option.id,
      label: `${option.label} ${count}`,
      hint: `${option.hint}, ${count} sample${count === 1 ? "" : "s"}`,
    };
  });

  const controls = (
    <View style={[styles.controls, compact ? styles.controlsCompact : null]}>
      <View style={styles.titleBlock}>
        <View style={[styles.titleRow, compact ? styles.titleRowCompact : null]}>
          <View
            style={[
              compact ? styles.titleIconCompact : styles.titleIcon,
              { backgroundColor: theme.colors.surfaceRaised, borderColor: theme.colors.border },
            ]}
          >
            <Icon name="tasks" size={compact ? 16 : 20} tone="accent" />
          </View>
          <View style={styles.titleCopy}>
            <View style={styles.titleLine}>
              <AppText
                variant={compact ? "body" : "title"}
                accessibilityRole="header"
                style={compact ? styles.titleCompact : undefined}
              >
                Tasks
              </AppText>
              <View
                style={[
                  styles.count,
                  compact ? styles.countCompact : null,
                  { backgroundColor: theme.colors.surfaceRaised },
                ]}
              >
                <AppText variant="caption" tone="textMuted">{`${total}`}</AppText>
              </View>
            </View>
            <AppText variant="caption" tone="textMuted" numberOfLines={compact ? 1 : undefined}>
              Sample tasks across your projects
            </AppText>
            {refreshNote ? (
              <AppText variant="caption" tone="textSubtle" numberOfLines={2}>
                {refreshNote}
              </AppText>
            ) : null}
          </View>
          <View style={styles.titleActions}>
            <IconButton
              name="refresh"
              size="sm"
              label={refreshing ? "Refreshing tasks" : "Refresh tasks"}
              onPress={refresh}
              disabled={refreshing}
            />
          </View>
        </View>
      </View>
      <SearchField
        label="Search tasks"
        placeholder="Search title, description, author, ref…"
        value={query}
        onChangeText={setQuery}
      />
      <View style={styles.pickerRow}>
        <SelectPopover
          label="Project"
          caption="PROJECTS"
          options={projectOptions}
          value={projectId}
          onChange={setProjectId}
          open={projectOpen}
          onOpenChange={setProjectOpen}
          searchable
          searchPlaceholder="Search projects"
        />
      </View>
      <SegmentedControl
        label="Task status"
        options={statusOptions}
        value={status}
        onChange={(next) => setStatus(parseTaskStatusFilter(next))}
      />
    </View>
  );

  if (showingDetail && selected) {
    return (
      <Screen maxWidth={1040} contentStyle={styles.content}>
        <View style={styles.backRow}>
          <Button label="Back to tasks" size="sm" variant="ghost" onPress={() => setSelectedId(null)} />
        </View>
        <TaskDetail snapshot={preview} desktopId={desktopId} taskId={selected.id} />
      </Screen>
    );
  }

  const list = refreshing ? (
    <LoadingState label="Loading tasks…" />
  ) : (
    <SectionList<PreviewTask, TaskSection>
      sections={sections}
      keyExtractor={(task) => task.id}
      style={styles.list}
      contentContainerStyle={styles.listContent}
      keyboardShouldPersistTaps="handled"
      renderSectionHeader={({ section }) => (
        <AppText variant="caption" tone="textMuted" style={styles.sectionTitle}>
          {section.title}
        </AppText>
      )}
      ListHeaderComponent={
        <View style={styles.controls}>
          {wide ? null : controls}
          <View style={styles.inboxHeader}>
            <AppText style={styles.inboxTitle}>Inbox</AppText>
            <AppText variant="caption" tone="textMuted">
              {`${allVisible.length} task${allVisible.length === 1 ? "" : "s"}`}
            </AppText>
          </View>
        </View>
      }
      renderItem={({ item }) => (
        <TaskListRow
          task={item}
          project={taskProjectName(preview, item.projectId)}
          selected={selected?.id === item.id}
          showChevron={!wide}
          onOpen={() => openTask(item.id)}
        />
      )}
      ListEmptyComponent={
        total === 0 ? (
          <EmptyState
            title="No tasks yet"
            message="Sample tasks for this machine will appear here."
          />
        ) : (
          <EmptyState
            title="No tasks match these filters"
            message="Try another project, status, or search."
            actionLabel="Show all"
            onAction={resetFilters}
          />
        )
      }
    />
  );

  return (
    <Screen maxWidth={1200} contentStyle={[styles.content, compact ? styles.contentCompact : null]}>
      <AppText variant="caption" tone="textMuted">
        {`Sample tasks on ${desktop.name}. Nothing syncs in this preview.`}
      </AppText>
      {wide ? controls : null}
      {wide ? (
        <View style={styles.split}>
          <View
            style={[
              styles.inboxPane,
              { width: width < 1100 ? 280 : 340, borderColor: theme.colors.border },
            ]}
          >
            {list}
          </View>
          <View style={styles.dividerWrap}>
            <View style={[styles.divider, { backgroundColor: theme.colors.border }]} />
          </View>
          <View style={styles.detailPane}>
            {selected ? (
              <TaskDetail key={selected.id} snapshot={preview} desktopId={desktopId} taskId={selected.id} />
            ) : (
              <View style={styles.chooseWrap}>
                <EmptyState
                  title="Choose a task"
                  message="Open a task to see its status, description, discussion and workspace action."
                />
              </View>
            )}
          </View>
        </View>
      ) : (
        list
      )}
    </Screen>
  );
}

const styles = StyleSheet.create({
  content: { gap: spacing.md },
  contentCompact: { gap: spacing.sm },
  controls: { gap: spacing.md },
  controlsCompact: { gap: spacing.sm },
  titleBlock: { gap: spacing.sm },
  titleRow: { flexDirection: "row", flexWrap: "wrap", alignItems: "center", gap: spacing.md },
  titleRowCompact: { gap: spacing.sm },
  titleActions: { flexDirection: "row", alignItems: "center", gap: spacing.sm, flexShrink: 0 },
  titleIcon: {
    width: 40,
    height: 40,
    borderRadius: 12,
    borderWidth: 1,
    alignItems: "center",
    justifyContent: "center",
    flexShrink: 0,
  },
  titleIconCompact: {
    width: 32,
    height: 32,
    borderRadius: 10,
    borderWidth: 1,
    alignItems: "center",
    justifyContent: "center",
    flexShrink: 0,
  },
  titleCopy: { flexGrow: 1, flexShrink: 1, flexBasis: 160, minWidth: 0, gap: 2 },
  titleLine: { flexDirection: "row", alignItems: "center", gap: spacing.sm },
  titleCompact: { fontWeight: "700" },
  count: {
    minWidth: 28,
    height: 24,
    paddingHorizontal: spacing.sm,
    borderRadius: 12,
    alignItems: "center",
    justifyContent: "center",
  },
  countCompact: { minWidth: 24, height: 20 },
  pickerRow: { flexDirection: "row", gap: 6 },
  backRow: { flexDirection: "row", flexWrap: "wrap", gap: spacing.sm },
  split: { flex: 1, minHeight: 0, flexDirection: "row" },
  inboxPane: { borderRightWidth: 1, minHeight: 0 },
  dividerWrap: { width: 1, minHeight: 0 },
  divider: { flex: 1, width: 1 },
  detailPane: { flex: 1, minWidth: 0, minHeight: 0 },
  chooseWrap: { flex: 1, padding: spacing.lg, justifyContent: "center" },
  list: { flex: 1, minHeight: 0 },
  listContent: { paddingBottom: spacing.xl, flexGrow: 1 },
  sectionTitle: { paddingTop: spacing.sm },
  inboxHeader: {
    flexDirection: "row",
    alignItems: "center",
    justifyContent: "space-between",
    paddingVertical: spacing.xs,
  },
  inboxTitle: { fontWeight: "600" },
  row: {
    minHeight: 56,
    flexDirection: "row",
    gap: spacing.sm,
    paddingVertical: spacing.sm,
    paddingHorizontal: spacing.sm,
    paddingLeft: spacing.md,
    borderBottomWidth: 1,
    borderLeftWidth: 2,
    alignItems: "flex-start",
  },
  copy: { flex: 1, minWidth: 0, gap: 2 },
  name: { fontWeight: "600" },
  metaRow: { flexDirection: "row", alignItems: "center", gap: 6 },
  metaAuthor: { flexShrink: 1 },
  trailing: { flexDirection: "row", alignItems: "center", gap: spacing.sm, flexShrink: 0 },
});
