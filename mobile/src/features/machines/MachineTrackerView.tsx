import { useCallback, useEffect, useMemo, useState } from "react";
import { FlatList, Linking, StyleSheet, View } from "react-native";
import { router } from "expo-router";
import {
  AppText,
  Button,
  EmptyState,
  FormField,
  LoadingState,
  SearchField,
  SelectPopover,
  StatusBadge,
  TabStrip,
} from "../../components";
import { useTheme } from "../../theme";
import { spacing } from "../../theme/tokens";
import { Feedback } from "../shared/Feedback";
import { useMachine } from "../../machines/MachineContext";
import { machineTaskPath } from "../../machines/routes";
import {
  createTrackerItem,
  importTrackerItem,
  listTrackerItems,
  moveTrackerItem,
  startTrackerItem,
} from "../../tasks/rpc";
import {
  TASK_STATUSES,
  TASK_STATUS_META,
  shortAge,
  type TaskListFilter,
  type TaskOptions,
  type TaskStatus,
  type Tracker,
  type TrackerItem,
} from "../../tasks/types";
import { AgentChips } from "./StartPickers";
import { MoveBar, statusesFor } from "./TaskMove";

const HOME = "";
/** Code hosts answer per repository: they need a project. */
const CODE_HOSTS = ["github", "gitlab", "forgejo"];
const STATUS_OPTIONS = [
  { id: "open", label: "Open" },
  { id: "closed", label: "Closed" },
  { id: "all", label: "All" },
] as const;

/**
 * A connected tracker's items, as the desktop's Tasks page shows them when
 * you switch to that tracker: open one in the tracker, start an agent on
 * it, save it as a task (or open the task it was saved as), move it to
 * another status, or file a new one. All through the desktop's own
 * connection; the tracker's keys stay there.
 */
export function MachineTrackerView({
  tracker,
  deviceId,
  options,
}: {
  tracker: Tracker;
  deviceId: string;
  options: TaskOptions | null;
}) {
  const { theme } = useTheme();
  const machine = useMachine();
  const rpc = machine.rpc;
  const projects = options?.projects ?? [];
  const [project, setProject] = useState<string | null>(null);
  const chosenProject = project ?? (CODE_HOSTS.includes(tracker.key) ? (projects[0]?.path ?? HOME) : HOME);
  const [items, setItems] = useState<readonly TrackerItem[] | null>(null);
  const [status, setStatus] = useState<TaskListFilter>("open");
  const [search, setSearch] = useState("");
  const [menu, setMenu] = useState<"project" | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [moving, setMoving] = useState<TrackerItem | null>(null);
  const [starting, setStarting] = useState<TrackerItem | null>(null);
  const [agent, setAgent] = useState<string | null>(null);
  const [form, setForm] = useState<{ title: string; body: string; place: string } | null>(null);

  const where = chosenProject === HOME ? null : chosenProject;
  const load = useCallback(() => {
    if (rpc === null) return () => undefined;
    let cancelled = false;
    setError(null);
    listTrackerItems(rpc, tracker.key, where).then(
      (list) => !cancelled && setItems(list),
      () => {
        if (cancelled) return;
        setItems((current) => current ?? []);
        setError(`Couldn't read ${tracker.label} through the desktop. Check it's online and still connected there.`);
      },
    );
    return () => {
      cancelled = true;
    };
  }, [rpc, tracker.key, tracker.label, where]);
  useEffect(() => {
    setItems(null);
    return load();
  }, [load]);

  const visible = useMemo(() => {
    const words = search.toLowerCase().split(/\s+/).filter(Boolean);
    const shown = statusesFor(status);
    return (items ?? [])
      .filter((item) => shown.includes(item.status))
      .filter((item) => words.every((word) => `${item.key} ${item.title}`.toLowerCase().includes(word)))
      .sort(
        (a, b) =>
          TASK_STATUSES.indexOf(a.status) - TASK_STATUSES.indexOf(b.status) || (b.updatedAt ?? 0) - (a.updatedAt ?? 0),
      );
  }, [items, status, search]);

  const replace = (item: TrackerItem) => setItems((list) => (list ?? []).map((known) => (known.id === item.id ? item : known)));
  const run = async (work: () => Promise<void>, failed: string) => {
    if (rpc === null) return;
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      await work();
    } catch {
      setError(failed);
    } finally {
      setBusy(false);
    }
  };

  const agents = options?.agents ?? [];
  const chosenAgent = agent ?? agents.find((entry) => entry.key !== "dragent")?.key ?? agents[0]?.key ?? null;

  return (
    <View style={styles.frame}>
      <View style={styles.controls}>
        <View style={styles.toolbar}>
          <SearchField
            label={`Search ${tracker.label}`}
            placeholder="Search key or title…"
            value={search}
            onChangeText={setSearch}
            style={styles.grow}
            autoCapitalize="none"
            autoCorrect={false}
          />
          <Button label="New item" size="sm" onPress={() => setForm({ title: "", body: "", place: "" })} />
        </View>
        <View style={styles.toolbar}>
          <SelectPopover
            label="Ask from"
            caption="PROJECT"
            options={[
              { id: HOME, label: "No project", hint: `${tracker.label}'s default` },
              ...projects.map((entry) => ({ id: entry.path, label: entry.label, hint: entry.path })),
            ]}
            value={chosenProject}
            onChange={setProject}
            open={menu === "project"}
            onOpenChange={(open) => setMenu(open ? "project" : null)}
            searchable={projects.length > 8}
            searchPlaceholder="Find a project"
          />
          <Button label="Refresh" size="sm" variant="ghost" disabled={busy} onPress={() => load()} />
        </View>
        <TabStrip label={`${tracker.label} status`} options={STATUS_OPTIONS} value={status} onChange={setStatus} />
        {form ? (
          <View style={[styles.panel, { borderColor: theme.colors.border, backgroundColor: theme.colors.surface }]}>
            <AppText style={styles.name}>{`New ${tracker.label} item`}</AppText>
            <FormField label="Title" value={form.title} onChangeText={(title) => setForm({ ...form, title })} maxLength={500} autoFocus />
            <FormField
              label="Description"
              placeholder="Optional"
              value={form.body}
              onChangeText={(body) => setForm({ ...form, body })}
              multiline
              maxLength={20_000}
            />
            <FormField
              label="Where"
              placeholder="Team, project or owner/repo — blank for the default"
              value={form.place}
              onChangeText={(place) => setForm({ ...form, place })}
              autoCapitalize="none"
              autoCorrect={false}
              maxLength={200}
            />
            <View style={styles.actions}>
              <Button label="Cancel" size="sm" variant="ghost" onPress={() => setForm(null)} />
              <Button
                label="Create"
                size="sm"
                loading={busy}
                disabled={busy || !form.title.trim()}
                onPress={() =>
                  void run(async () => {
                    const item = await createTrackerItem(rpc!, {
                      provider: tracker.key,
                      project: where,
                      title: form.title,
                      body: form.body,
                      place: form.place,
                    });
                    setItems((list) => [item, ...(list ?? []).filter((known) => known.id !== item.id)]);
                    setForm(null);
                    setNote(`Created ${item.key} in ${tracker.label}.`);
                  }, `It wasn't created in ${tracker.label}. Check "Where", then try again.`)
                }
              />
            </View>
          </View>
        ) : null}
        {moving ? (
          <MoveBar
            label={`Move ${moving.key} to`}
            current={moving.status}
            statuses={TASK_STATUSES}
            busy={busy}
            onMove={(to: TaskStatus) =>
              void run(async () => {
                const moved = await moveTrackerItem(rpc!, { provider: tracker.key, project: where, item: moving.id, status: to });
                replace(moved);
                setMoving(null);
              }, `${tracker.label} didn't take that change. It may have no such state.`)
            }
            onCancel={() => setMoving(null)}
          />
        ) : null}
        {starting ? (
          <View style={[styles.panel, { borderColor: theme.colors.border, backgroundColor: theme.colors.surface }]}>
            <AppText style={styles.name} numberOfLines={2}>{`Start ${starting.key}: ${starting.title}`}</AppText>
            <AppText variant="caption" tone="textMuted">
              An agent works on it in a new workspace on the desktop, in its repository or the project above.
            </AppText>
            <AgentChips label="Agent" agents={agents} selected={chosenAgent ? [chosenAgent] : []} onChange={(next) => setAgent(next[0] ?? null)} single />
            <View style={styles.actions}>
              <Button label="Cancel" size="sm" variant="ghost" onPress={() => setStarting(null)} />
              <Button
                label="Start"
                size="sm"
                loading={busy}
                disabled={busy || !chosenAgent}
                onPress={() =>
                  void run(async () => {
                    const result = await startTrackerItem(rpc!, { provider: tracker.key, project: where, item: starting.id, agent: chosenAgent! });
                    if (result.started) {
                      setNote(`Started on the desktop: a new workspace is opening for ${starting.key}.`);
                      setStarting(null);
                    } else {
                      setError(result.reason ?? "The desktop didn't start it.");
                    }
                  }, "The desktop didn't answer. Check it's online, then try again.")
                }
              />
            </View>
          </View>
        ) : null}
        {note ? <AppText variant="caption" tone="textMuted">{note}</AppText> : null}
        <Feedback message={error} />
      </View>
      {items === null ? (
        <LoadingState label={`Loading ${tracker.label}…`} />
      ) : (
        <FlatList
          data={visible}
          keyExtractor={(item) => item.id}
          style={styles.frame}
          contentContainerStyle={[styles.list, visible.length === 0 ? styles.emptyList : undefined]}
          renderItem={({ item }) => {
            const meta = TASK_STATUS_META[item.status];
            return (
              <View style={[styles.row, { borderColor: theme.colors.border }]}>
                <View style={styles.rowTop}>
                  <View style={styles.grow}>
                    <AppText style={styles.name} numberOfLines={2}>{item.title}</AppText>
                    <AppText variant="caption" tone="textMuted" numberOfLines={1}>
                      {[item.key, item.stateLabel, item.assignee, item.updatedAt ? shortAge(item.updatedAt) : null].filter(Boolean).join(" · ")}
                    </AppText>
                  </View>
                  <StatusBadge label={meta.label} tone={meta.tone} />
                </View>
                <View style={styles.rowActions}>
                  {item.url ? <Button label="Open" size="sm" variant="ghost" onPress={() => void Linking.openURL(item.url!)} /> : null}
                  <Button label="Start" size="sm" variant="ghost" disabled={busy} onPress={() => setStarting(item)} />
                  {item.importedTask ? (
                    <Button
                      label={`Open ${item.importedTask}`}
                      size="sm"
                      variant="ghost"
                      onPress={() => router.push(machineTaskPath(deviceId, item.importedTask!))}
                    />
                  ) : (
                    <Button
                      label="Import as task"
                      size="sm"
                      variant="ghost"
                      disabled={busy}
                      onPress={() =>
                        void run(async () => {
                          const task = await importTrackerItem(rpc!, { provider: tracker.key, project: where, item: item.id });
                          replace({ ...item, importedTask: task.key });
                          setNote(`Saved as ${task.key}.`);
                        }, "It wasn't imported. Try again.")
                      }
                    />
                  )}
                  <Button label="Move" size="sm" variant="ghost" disabled={busy} onPress={() => setMoving(item)} />
                </View>
              </View>
            );
          }}
          ListEmptyComponent={
            <EmptyState
              title={search ? "No items match" : `Nothing in ${tracker.label} here`}
              message={
                CODE_HOSTS.includes(tracker.key) && where === null
                  ? `${tracker.label} answers per repository: choose a project above.`
                  : "Try another status or project."
              }
            />
          }
        />
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  frame: { flex: 1, minHeight: 0, gap: spacing.sm },
  controls: { gap: spacing.sm, paddingHorizontal: spacing.md },
  toolbar: { flexDirection: "row", alignItems: "center", gap: spacing.sm, flexWrap: "wrap" },
  grow: { flex: 1, minWidth: 0 },
  name: { fontWeight: "600" },
  panel: { gap: spacing.sm, padding: spacing.sm, borderRadius: 10, borderWidth: StyleSheet.hairlineWidth },
  actions: { flexDirection: "row", justifyContent: "flex-end", gap: spacing.sm },
  list: { paddingBottom: 24 },
  emptyList: { flexGrow: 1 },
  row: { gap: 4, paddingVertical: spacing.sm, paddingHorizontal: spacing.md, borderBottomWidth: StyleSheet.hairlineWidth },
  rowTop: { flexDirection: "row", alignItems: "flex-start", gap: spacing.sm },
  rowActions: { flexDirection: "row", flexWrap: "wrap", gap: spacing.xs },
});
