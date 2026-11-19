import { useEffect, useMemo, useState } from "react";
import { FlatList, ScrollView, StyleSheet, View } from "react-native";
import { router } from "expo-router";
import {
  AppText,
  Avatar,
  Button,
  Control,
  EmptyState,
  Icon,
  LoadingState,
  SearchField,
  SelectPopover,
  StatusBadge,
  TabStrip,
  OptionRow,
} from "../../components";
import { useTheme } from "../../theme";
import { spacing } from "../../theme/tokens";
import { Feedback } from "../shared/Feedback";
import { useMachine } from "../../machines/MachineContext";
import { machineNewTaskPath, machineTaskPath } from "../../machines/routes";
import { useLiveTaskList } from "../../tasks/useLiveTasks";
import { listTrackers, startTasks, taskOptions, updateTask } from "../../tasks/rpc";
import {
  TASK_PRIORITY_LABEL,
  TASK_STATUSES,
  TASK_STATUS_META,
  dueLabel,
  shortAge,
  type LiveTask,
  type TaskListFilter,
  type TaskStatus,
  type Tracker,
} from "../../tasks/types";
import { AgentChips, useStartOptions } from "./StartPickers";
import { MachineTrackerView } from "./MachineTrackerView";
import { MoveBar, statusesFor } from "./TaskMove";

const STATUS_OPTIONS = [
  { id: "open", label: "Open", hint: "Backlog, todo, in progress and in review" },
  { id: "closed", label: "Closed", hint: "Done and canceled" },
  { id: "all", label: "All", hint: "Every task" },
] as const;

const LAYOUT_OPTIONS = [
  { id: "list", label: "List", hint: "Tasks grouped by status" },
  { id: "board", label: "Board", hint: "A column for each status" },
] as const;

const EVERYONE = "__everyone__";
const ANY_LABEL = "__any__";
const DRAGENT = "__dragent__";

/** One task: title, then key, project, priority and comments, then who and when. */
function TaskRow({
  task,
  selecting,
  selected,
  onOpen,
  onToggle,
}: {
  task: LiveTask;
  selecting: boolean;
  selected: boolean;
  onOpen: () => void;
  onToggle: () => void;
}) {
  const { theme } = useTheme();
  const status = TASK_STATUS_META[task.status];
  const bits = [
    task.project,
    task.priority !== "none" ? TASK_PRIORITY_LABEL[task.priority] : null,
    task.dueAt ? dueLabel(task.dueAt) : null,
    task.commentCount > 0 ? `${task.commentCount} comment${task.commentCount === 1 ? "" : "s"}` : null,
  ].filter(Boolean);
  return (
    <Control
      accessibilityRole={selecting ? "checkbox" : "button"}
      {...(selecting ? { accessibilityState: { checked: selected } } : {})}
      accessibilityLabel={`${task.key}, ${task.title}, ${status.label}${task.assignee ? `, assigned to ${task.assignee.name}` : ""}`}
      accessibilityHint={selecting ? undefined : "Long press to select several"}
      onPress={selecting ? onToggle : onOpen}
      onLongPress={onToggle}
      style={({ pressed, hovered }) => [
        styles.row,
        {
          borderColor: theme.colors.border,
          backgroundColor: selected ? theme.colors.surfaceRaised : pressed || hovered ? theme.colors.surface : undefined,
        },
      ]}
    >
      {selecting ? (
        <View style={[styles.check, { borderColor: selected ? theme.colors.accent : theme.colors.border, backgroundColor: selected ? theme.colors.accent : undefined }]}>
          {selected ? <Icon name="check" size={12} tone="onAccent" /> : null}
        </View>
      ) : (
        <Icon name="tasks" size={16} tone={task.status === "done" || task.status === "canceled" ? "textMuted" : "accent"} />
      )}
      <View style={styles.copy}>
        <AppText style={styles.name} numberOfLines={2}>{task.title}</AppText>
        <AppText variant="caption" tone="textMuted" numberOfLines={1}>{[task.key, ...bits].join(" · ")}</AppText>
        <View style={styles.metaRow}>
          {task.assignee ? (
            <>
              <Avatar name={task.assignee.name} size={16} />
              <AppText variant="caption" tone="textMuted" numberOfLines={1} style={styles.metaName}>{task.assignee.name}</AppText>
            </>
          ) : (
            <AppText variant="caption" tone="textSubtle">Unassigned</AppText>
          )}
          <AppText variant="caption" tone="textSubtle">{shortAge(task.updatedAt)}</AppText>
        </View>
      </View>
      {selecting ? null : <Icon name="chevron-right" size={16} />}
    </Control>
  );
}

/** A board column's card. A long press picks it up to move. */
function BoardCard({ task, moving, onOpen, onMove }: { task: LiveTask; moving: boolean; onOpen: () => void; onMove: () => void }) {
  const { theme } = useTheme();
  return (
    <Control
      accessibilityRole="button"
      accessibilityLabel={`${task.key}, ${task.title}${task.assignee ? `, assigned to ${task.assignee.name}` : ""}`}
      accessibilityHint="Long press to move it to another status"
      onPress={onOpen}
      onLongPress={onMove}
      style={({ pressed, hovered }) => [
        styles.card,
        {
          borderColor: moving ? theme.colors.accent : theme.colors.border,
          backgroundColor: pressed || hovered ? theme.colors.surfaceRaised : theme.colors.surface,
        },
      ]}
    >
      <AppText numberOfLines={3} style={styles.name}>{task.title}</AppText>
      <View style={styles.metaRow}>
        <AppText variant="caption" tone="textMuted">{task.key}</AppText>
        {task.priority !== "none" ? <AppText variant="caption" tone="textMuted">{TASK_PRIORITY_LABEL[task.priority]}</AppText> : null}
        <View style={styles.spacer} />
        {task.assignee ? <Avatar name={task.assignee.name} size={16} /> : null}
      </View>
    </Control>
  );
}

function TaskBoard({
  tasks,
  filter,
  moving,
  onOpen,
  onMove,
}: {
  tasks: readonly LiveTask[];
  filter: TaskListFilter;
  moving: string | null;
  onOpen: (task: LiveTask) => void;
  onMove: (task: LiveTask) => void;
}) {
  const { theme } = useTheme();
  return (
    <ScrollView horizontal style={styles.frame} contentContainerStyle={styles.board} showsHorizontalScrollIndicator>
      {statusesFor(filter).map((key) => {
        const inColumn = tasks.filter((task) => task.status === key);
        const meta = TASK_STATUS_META[key];
        return (
          <View key={key} style={[styles.column, { backgroundColor: theme.colors.background, borderColor: theme.colors.border }]}>
            <View style={styles.columnHeader}>
              <StatusBadge label={meta.label} tone={meta.tone} />
              <AppText variant="caption" tone="textMuted">{String(inColumn.length)}</AppText>
            </View>
            <ScrollView contentContainerStyle={styles.columnBody}>
              {inColumn.length === 0 ? (
                <AppText variant="caption" tone="textSubtle">Nothing here</AppText>
              ) : (
                inColumn.map((task) => (
                  <BoardCard key={task.id} task={task} moving={moving === task.id} onOpen={() => onOpen(task)} onMove={() => onMove(task)} />
                ))
              )}
            </ScrollView>
          </View>
        );
      })}
    </ScrollView>
  );
}

type Row = { kind: "header"; status: TaskStatus; count: number } | { kind: "task"; task: LiveTask };

/**
 * Live Tasks for the machine hub, with what the desktop's Tasks page does:
 * the organisation's tasks (filter by status, assignee and label; a list
 * grouped by status, or a board), several at once (move or start them),
 * and the items of each connected tracker.
 */
export function MachineTasksSection({ deviceId }: { deviceId: string }) {
  const { theme } = useTheme();
  const machine = useMachine();
  const rpc = machine.rpc;
  const { options } = useStartOptions(rpc, taskOptions);
  const [trackers, setTrackers] = useState<readonly Tracker[]>([]);
  const [source, setSource] = useState(DRAGENT);
  const [status, setStatus] = useState<TaskListFilter>("open");
  const [assignee, setAssignee] = useState(EVERYONE);
  const [label, setLabel] = useState(ANY_LABEL);
  const [layout, setLayout] = useState<"list" | "board">("list");
  const [menu, setMenu] = useState<"source" | "assignee" | "label" | "bulk-status" | "bulk-project" | null>(null);
  const [draft, setDraft] = useState("");
  const [query, setQuery] = useState("");
  const [collapsed, setCollapsed] = useState<ReadonlySet<TaskStatus>>(new Set());
  const [selecting, setSelecting] = useState(false);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [moving, setMoving] = useState<LiveTask | null>(null);
  const [startSheet, setStartSheet] = useState(false);
  const [startAgent, setStartAgent] = useState<string | null>(null);
  const [startProject, setStartProject] = useState<string>("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);

  useEffect(() => {
    if (rpc === null) return;
    let cancelled = false;
    listTrackers(rpc).then((list) => !cancelled && setTrackers(list), () => undefined);
    return () => {
      cancelled = true;
    };
  }, [rpc]);
  // Search after a pause in typing, not on every keystroke.
  useEffect(() => {
    const timer = setTimeout(() => setQuery(draft.trim()), 400);
    return () => clearTimeout(timer);
  }, [draft]);
  const live = useLiveTaskList({
    status,
    q: query,
    assignee: assignee === EVERYONE ? "" : assignee,
    label: label === ANY_LABEL ? "" : label,
  });
  const tasks = useMemo(() => (live.state.status === "live" ? live.state.result.tasks : []), [live.state]);

  const rows = useMemo<Row[]>(() => {
    const out: Row[] = [];
    for (const key of statusesFor(status)) {
      const inGroup = tasks.filter((task) => task.status === key);
      if (inGroup.length === 0) continue;
      out.push({ kind: "header", status: key, count: inGroup.length });
      if (!collapsed.has(key)) for (const task of inGroup) out.push({ kind: "task", task });
    }
    return out;
  }, [tasks, status, collapsed]);

  const sourceOptions = [
    { id: DRAGENT, label: "Dragent tasks", hint: "Your organisation's tasks" },
    ...trackers.map((tracker) => ({ id: tracker.key, label: tracker.label, hint: `Items in ${tracker.label}` })),
  ];
  const sourcePicker =
    trackers.length > 0 ? (
      <SelectPopover
        label="Show"
        caption="SHOW"
        options={sourceOptions}
        value={source}
        onChange={(next) => {
          setSource(next);
          setSelecting(false);
          setSelected(new Set());
          setMoving(null);
        }}
        open={menu === "source"}
        onOpenChange={(open) => setMenu(open ? "source" : null)}
      />
    ) : null;

  const tracker = trackers.find((entry) => entry.key === source);
  if (tracker) {
    return (
      <View style={styles.frame}>
        <View style={styles.controls}>{sourcePicker}</View>
        <MachineTrackerView tracker={tracker} deviceId={deviceId} options={options} />
      </View>
    );
  }

  const toggle = (task: LiveTask) => {
    setSelecting(true);
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(task.id)) next.delete(task.id);
      else next.add(task.id);
      return next;
    });
  };
  const chosen = tasks.filter((task) => selected.has(task.id));
  const endSelection = () => {
    setSelecting(false);
    setSelected(new Set());
    setStartSheet(false);
  };

  const moveTasks = async (list: readonly LiveTask[], to: TaskStatus) => {
    if (rpc === null) return;
    setMenu(null);
    setBusy(true);
    setError(null);
    let failed = 0;
    for (const task of list) {
      if (task.status === to) continue;
      try {
        await updateTask(rpc, task.key, { status: to });
      } catch {
        failed += 1;
      }
    }
    setBusy(false);
    setMoving(null);
    if (failed > 0) setError(`${failed} of ${list.length} didn't move. Try again.`);
    live.retry();
  };

  const agents = options?.agents ?? [];
  const chosenAgent = startAgent ?? agents.find((agent) => agent.key !== "dragent")?.key ?? agents[0]?.key ?? null;
  const startChosen = async () => {
    if (rpc === null || !chosenAgent) return;
    setBusy(true);
    setError(null);
    setStartSheet(false);
    const keys = chosen.map((task) => task.key);
    const failures: string[] = [];
    let done = 0;
    setNote(`Starting ${keys.length} task${keys.length === 1 ? "" : "s"} on the desktop, one after another…`);
    try {
      await startTasks(
        rpc,
        { tasks: keys, agent: chosenAgent, ...(startProject ? { project: startProject } : {}) },
        (key, result) => {
          done += 1;
          if (!result.started) failures.push(`${key}: ${result.reason ?? "didn't start"}`);
          setNote(`Started ${done - failures.length} of ${keys.length}…`);
        },
      );
      setNote(`Started ${keys.length - failures.length} of ${keys.length} on the desktop.`);
      if (failures.length > 0) setError(failures.join("\n"));
      endSelection();
    } catch {
      setError("The desktop stopped answering. Check it's online; tasks already started keep going.");
    } finally {
      setBusy(false);
    }
  };

  const members = options?.members ?? [];
  const assigneeOptions = [
    { id: EVERYONE, label: "Everyone" },
    { id: "me", label: "Assigned to me" },
    { id: "none", label: "Unassigned" },
    ...members.map((member) => ({ id: member.id, label: member.name })),
  ];
  const labelOptions = [{ id: ANY_LABEL, label: "Any label" }, ...(options?.labels ?? []).map((name) => ({ id: name, label: name }))];

  const controls = (
    <View style={styles.controls}>
      <View style={styles.toolbar}>
        <SearchField
          label="Search tasks"
          placeholder="Search key, title or description…"
          value={draft}
          onChangeText={setDraft}
          style={styles.search}
          autoCapitalize="none"
          autoCorrect={false}
        />
        <Button label="New task" size="sm" onPress={() => router.push(machineNewTaskPath(deviceId))} />
      </View>
      <TabStrip label="Task status" options={STATUS_OPTIONS} value={status} onChange={setStatus} />
      <OptionRow accessibilityLabel="Filters">
        {sourcePicker}
        <SelectPopover
          label="Assignee"
          caption="ASSIGNEE"
          options={assigneeOptions}
          value={assignee}
          onChange={setAssignee}
          open={menu === "assignee"}
          onOpenChange={(open) => setMenu(open ? "assignee" : null)}
          searchable={assigneeOptions.length > 8}
          searchPlaceholder="Find a member"
        />
        <SelectPopover
          label="Label"
          caption="LABEL"
          options={labelOptions}
          value={label}
          onChange={setLabel}
          open={menu === "label"}
          onOpenChange={(open) => setMenu(open ? "label" : null)}
          searchable={labelOptions.length > 8}
          searchPlaceholder="Find a label"
        />
        <Button
          label={selecting ? "Done" : "Select"}
          size="sm"
          variant="ghost"
          onPress={() => (selecting ? endSelection() : setSelecting(true))}
        />
      </OptionRow>
      <TabStrip
        label="Layout"
        options={LAYOUT_OPTIONS}
        value={layout}
        onChange={(next) => {
          setLayout(next);
          setMoving(null);
        }}
      />
      {selecting ? (
        <View style={[styles.selectionBar, { borderColor: theme.colors.border, backgroundColor: theme.colors.surface }]}>
          <AppText style={styles.name}>{`${chosen.length} selected`}</AppText>
          <View style={styles.spacer} />
          <SelectPopover
            label="Move to"
            caption="MOVE TO"
            options={TASK_STATUSES.map((key) => ({ id: key, label: TASK_STATUS_META[key].label }))}
            value={chosen[0]?.status ?? "todo"}
            onChange={(to) => void moveTasks(chosen, to)}
            open={menu === "bulk-status"}
            onOpenChange={(open) => setMenu(open && chosen.length > 0 ? "bulk-status" : null)}
            triggerLabel="Move to"
          />
          <Button label="Start" size="sm" disabled={busy || chosen.length === 0} onPress={() => setStartSheet(true)} />
          <Button label="Clear" size="sm" variant="ghost" disabled={chosen.length === 0} onPress={() => setSelected(new Set())} />
        </View>
      ) : null}
      {startSheet ? (
        <View style={[styles.selectionBar, styles.sheet, { borderColor: theme.colors.border, backgroundColor: theme.colors.surface }]}>
          <AppText style={styles.name}>{`Start ${chosen.length} task${chosen.length === 1 ? "" : "s"}`}</AppText>
          <AppText variant="caption" tone="textMuted">
            Each gets its own workspace on the desktop, one after another, in its own project unless you pick one.
          </AppText>
          {options === null ? (
            <LoadingState label="Loading the desktop's agents…" />
          ) : (
            <>
              <SelectPopover
                label="Project"
                caption="WORK IN"
                options={[
                  { id: "", label: "Each task's own project" },
                  ...options.projects.map((project) => ({ id: project.path, label: project.label, hint: project.path })),
                ]}
                value={startProject}
                onChange={setStartProject}
                open={menu === "bulk-project"}
                onOpenChange={(open) => setMenu(open ? "bulk-project" : null)}
              />
              <AgentChips
                label="Agent"
                agents={agents}
                selected={chosenAgent ? [chosenAgent] : []}
                onChange={(next) => setStartAgent(next[0] ?? null)}
                single
              />
            </>
          )}
          <View style={styles.actions}>
            <Button label="Cancel" size="sm" variant="ghost" onPress={() => setStartSheet(false)} />
            <Button label="Start" size="sm" disabled={busy || !chosenAgent} onPress={() => void startChosen()} />
          </View>
        </View>
      ) : null}
      {moving ? (
        <MoveBar
          label={`Move ${moving.key} to`}
          current={moving.status}
          statuses={TASK_STATUSES}
          busy={busy}
          onMove={(to) => void moveTasks([moving], to)}
          onCancel={() => setMoving(null)}
        />
      ) : null}
      {note ? <AppText variant="caption" tone="textMuted">{note}</AppText> : null}
      <Feedback message={error} />
    </View>
  );

  if (live.state.status === "loading") {
    return (
      <View style={styles.frame}>
        {controls}
        <LoadingState label="Loading tasks…" />
      </View>
    );
  }
  if (live.state.status === "unavailable") {
    const ended = live.state.action === "reconnect";
    return (
      <View style={styles.frame}>
        {controls}
        <View style={styles.padded}>
          <Feedback message={live.state.reason} />
          <Button label={ended ? "Start new session" : "Retry"} size="sm" variant="ghost" onPress={ended ? live.reconnect : live.retry} />
        </View>
      </View>
    );
  }

  const { truncated } = live.state.result;
  const open = (task: LiveTask) => router.push(machineTaskPath(deviceId, task.key));
  const filtered = query || assignee !== EVERYONE || label !== ANY_LABEL;
  return (
    <View style={styles.frame}>
      {controls}
      {truncated ? (
        <AppText variant="caption" tone="textMuted" style={styles.padX}>
          Showing the 200 most recently changed. Search or filter to narrow it down.
        </AppText>
      ) : null}
      {layout === "board" && tasks.length > 0 ? (
        <TaskBoard tasks={tasks} filter={status} moving={moving?.id ?? null} onOpen={open} onMove={setMoving} />
      ) : (
        <FlatList
          data={rows}
          keyExtractor={(row) => (row.kind === "header" ? `h-${row.status}` : row.task.id)}
          style={styles.frame}
          contentContainerStyle={[styles.list, rows.length === 0 ? styles.emptyList : undefined]}
          renderItem={({ item: row }) =>
            row.kind === "header" ? (
              <Control
                accessibilityRole="button"
                accessibilityState={{ expanded: !collapsed.has(row.status) }}
                accessibilityLabel={`${TASK_STATUS_META[row.status].label}, ${row.count} tasks`}
                onPress={() =>
                  setCollapsed((current) => {
                    const next = new Set(current);
                    if (next.has(row.status)) next.delete(row.status);
                    else next.add(row.status);
                    return next;
                  })
                }
                style={[styles.groupHeader, { backgroundColor: theme.colors.background, borderColor: theme.colors.border }]}
              >
                <Icon name={collapsed.has(row.status) ? "chevron-right" : "chevron-down"} size={14} />
                <StatusBadge label={TASK_STATUS_META[row.status].label} tone={TASK_STATUS_META[row.status].tone} />
                <AppText variant="caption" tone="textMuted">{String(row.count)}</AppText>
              </Control>
            ) : (
              <TaskRow
                task={row.task}
                selecting={selecting}
                selected={selected.has(row.task.id)}
                onOpen={() => open(row.task)}
                onToggle={() => toggle(row.task)}
              />
            )
          }
          ListEmptyComponent={
            filtered ? (
              <EmptyState
                title="No tasks match"
                message="Try other words, another filter or another status."
                actionLabel="Clear filters"
                onAction={() => {
                  setDraft("");
                  setAssignee(EVERYONE);
                  setLabel(ANY_LABEL);
                }}
              />
            ) : (
              <EmptyState
                title={status === "closed" ? "No closed tasks" : "No open tasks"}
                message="Tasks your organisation creates on the desktop or here appear in this list."
                actionLabel="New task"
                onAction={() => router.push(machineNewTaskPath(deviceId))}
              />
            )
          }
        />
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  frame: { flex: 1, minHeight: 0, gap: spacing.sm },
  controls: { gap: spacing.sm, paddingHorizontal: spacing.md, paddingTop: spacing.sm },
  toolbar: { flexDirection: "row", alignItems: "center", gap: spacing.sm },
  search: { flex: 1, minWidth: 0 },
  list: { paddingBottom: 24 },
  emptyList: { flexGrow: 1 },
  padded: { gap: 12, padding: 16 },
  padX: { paddingHorizontal: spacing.md },
  row: {
    flexDirection: "row",
    alignItems: "flex-start",
    gap: spacing.sm,
    paddingVertical: spacing.sm,
    paddingHorizontal: spacing.md,
    borderBottomWidth: StyleSheet.hairlineWidth,
    minHeight: 44,
  },
  check: { width: 18, height: 18, borderRadius: 4, borderWidth: 1.5, alignItems: "center", justifyContent: "center", marginTop: 2 },
  groupHeader: {
    flexDirection: "row",
    alignItems: "center",
    gap: spacing.sm,
    minHeight: 40,
    paddingHorizontal: spacing.md,
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
  copy: { flex: 1, minWidth: 0, gap: 2 },
  name: { fontWeight: "600" },
  metaRow: { flexDirection: "row", alignItems: "center", gap: 6, minWidth: 0 },
  metaName: { flexShrink: 1, minWidth: 0 },
  board: { gap: spacing.sm, paddingHorizontal: spacing.md, paddingBottom: 24 },
  column: { width: 260, borderRadius: 12, borderWidth: StyleSheet.hairlineWidth, padding: spacing.sm, gap: spacing.sm },
  columnHeader: { flexDirection: "row", alignItems: "center", justifyContent: "space-between" },
  columnBody: { gap: spacing.sm, paddingBottom: spacing.sm },
  card: { gap: 6, padding: spacing.sm, borderRadius: 10, borderWidth: 1 },
  spacer: { flex: 1 },
  chips: { flexDirection: "row", flexWrap: "wrap", gap: spacing.xs },
  moveBar: { gap: spacing.xs, padding: spacing.sm, borderRadius: 10, borderWidth: 1 },
  selectionBar: {
    flexDirection: "row",
    flexWrap: "wrap",
    alignItems: "center",
    gap: spacing.xs,
    padding: spacing.sm,
    borderRadius: 10,
    borderWidth: StyleSheet.hairlineWidth,
  },
  sheet: { flexDirection: "column", alignItems: "stretch" },
  actions: { flexDirection: "row", justifyContent: "flex-end", gap: spacing.sm },
});
