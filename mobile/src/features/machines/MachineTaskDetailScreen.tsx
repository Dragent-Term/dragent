import { useMemo, useState } from "react";
import { Linking, StyleSheet, View } from "react-native";
import { router } from "expo-router";
import * as Clipboard from "expo-clipboard";
import {
  AppText,
  Avatar,
  Button,
  Card,
  ConfirmationPanel,
  Control,
  FormField,
  Icon,
  IconButton,
  LoadingState,
  OptionRow,
  SelectPopover,
  StatusBadge,
} from "../../components";
import { ScrollScreen } from "../../layout";
import { useTheme } from "../../theme";
import { spacing } from "../../theme/tokens";
import { useMachine } from "../../machines/MachineContext";
import { machinePath, machineWorkspacePath } from "../../machines/routes";
import { workspaceKey } from "../../machines/machineCatalog";
import { DueDatePicker } from "./DueDatePicker";
import { MachineHeader } from "./MachineHeader";
import { Feedback } from "../shared/Feedback";
import { AgentChips, useStartOptions } from "./StartPickers";
import { useLiveTaskDetail } from "../../tasks/useLiveTasks";
import {
  commentOnTask,
  deleteComment,
  deleteTask,
  editComment,
  linkToTask,
  restoreTask,
  startTask,
  taskOptions,
  unlinkFromTask,
  updateTask,
  type TaskChange,
} from "../../tasks/rpc";
import {
  TASK_PRIORITIES,
  TASK_PRIORITY_LABEL,
  TASK_STATUSES,
  TASK_STATUS_META,
  activityLine,
  dueLabel,
  shortAge,
  type LiveTask,
  type TaskComment,
  type TaskLink,
} from "../../tasks/types";

const NOBODY = "__nobody__";
const NO_PROJECT = "__none__";
type Menu = "status" | "priority" | "assignee" | "project" | "due" | "start-project" | null;

/**
 * One of the organisation's tasks, live through the paired desktop, with
 * everything the desktop's task page does: edit it, set its status,
 * priority, assignee, labels, project and due date, link pull requests and
 * tracker items, discuss it (and edit or delete your own comments), start
 * an agent on it on the desktop, and delete it (with a way back).
 */
export function MachineTaskDetailScreen({ deviceId, task: taskRef }: { deviceId: string; task: string }) {
  const { theme } = useTheme();
  const machine = useMachine();
  const live = useLiveTaskDetail(taskRef);
  const rpc = machine.rpc;
  const { options } = useStartOptions(rpc, taskOptions);
  const [changed, setChanged] = useState<LiveTask | null>(null);
  const [posted, setPosted] = useState<TaskComment[]>([]);
  const [edited, setEdited] = useState<Record<string, TaskComment | null>>({});
  const [menu, setMenu] = useState<Menu>(null);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<{ title: string; description: string } | null>(null);
  const [commentEdit, setCommentEdit] = useState<{ id: string; body: string } | null>(null);
  const [labelDraft, setLabelDraft] = useState("");
  const [linkDraft, setLinkDraft] = useState("");
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [deleted, setDeleted] = useState(false);
  const [startOpen, setStartOpen] = useState(false);
  const [startProject, setStartProject] = useState<string | null>(null);
  const [startAgent, setStartAgent] = useState<string | null>(null);
  const [startNote, setStartNote] = useState<string | null>(null);
  const [copied, setCopied] = useState<string | null>(null);

  const back = machinePath(deviceId);
  const toList = () => router.navigate({ pathname: back.pathname, params: { ...back.params, view: "tasks" } });
  const fromServer = live.state.status === "live" ? live.state.result : null;
  // The newer of the last refresh and the last change made here.
  const task = useMemo(() => {
    if (!fromServer) return changed;
    return changed && changed.updatedAt >= fromServer.task.updatedAt ? changed : fromServer.task;
  }, [fromServer, changed]);
  const comments = useMemo(() => {
    const known = new Set(fromServer?.comments.map((comment) => comment.id) ?? []);
    return [...(fromServer?.comments ?? []), ...posted.filter((comment) => !known.has(comment.id))]
      .map((comment) => (comment.id in edited ? edited[comment.id] : comment))
      .filter((comment): comment is TaskComment => comment !== null && comment !== undefined);
  }, [fromServer, posted, edited]);

  const header = (
    <MachineHeader
      title={task ? task.key : "Task"}
      subtitle={task ? task.title : "Tasks"}
      online={machine.catalog.status === "live" ? machine.catalog.machine.online : null}
      onBack={toList}
      backLabel="Back to Tasks"
      actions={
        task && !deleted ? (
          <View style={styles.headerActions}>
            <IconButton
              name="copy"
              label={`Copy ${task.key}`}
              onPress={() => {
                void Clipboard.setStringAsync(task.key).then(() => setCopied(`Copied ${task.key}`));
              }}
            />
            <IconButton
              name="link"
              label="Copy the task's key and title"
              onPress={() => {
                void Clipboard.setStringAsync(`${task.key}: ${task.title}`).then(() => setCopied("Copied the key and title"));
              }}
            />
            {rpc !== null && editing === null ? (
              <IconButton
                name="pencil"
                label="Edit title and description"
                onPress={() => setEditing({ title: task.title, description: task.description })}
              />
            ) : null}
          </View>
        ) : undefined
      }
    />
  );

  if (!task) {
    if (live.state.status === "unavailable") {
      const ended = live.state.action === "reconnect";
      return (
        <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
          {header}
          <View style={styles.padded}>
            <Feedback message={live.state.reason} />
            <Button label={ended ? "Start new session" : "Retry"} size="sm" variant="ghost" onPress={ended ? live.reconnect : live.retry} />
          </View>
        </View>
      );
    }
    return (
      <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
        {header}
        <LoadingState label="Loading the task…" />
      </View>
    );
  }

  /** Run a change; true when it went through. */
  const run = async <T,>(work: () => Promise<T>, failed: string, done?: (value: T) => void): Promise<boolean> => {
    if (rpc === null) return false;
    setMenu(null);
    setBusy(true);
    setError(null);
    try {
      const value = await work();
      done?.(value);
      return true;
    } catch {
      setError(failed);
      return false;
    } finally {
      setBusy(false);
    }
  };
  const change = (update: TaskChange) =>
    run(() => updateTask(rpc!, task.key, update), "That change didn't go through. Check the desktop is online and signed in, then try again.", setChanged);

  if (deleted) {
    return (
      <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
        {header}
        <View style={styles.padded}>
          <AppText variant="title">{`${task.key} was deleted`}</AppText>
          <AppText tone="textMuted">It's gone for everyone in your organisation. You can still bring it back.</AppText>
          <Feedback message={error} />
          <View style={styles.actions}>
            <Button label="Back to Tasks" size="sm" variant="ghost" onPress={toList} />
            <Button
              label="Restore"
              size="sm"
              loading={busy}
              disabled={busy}
              onPress={() =>
                void run(() => restoreTask(rpc!, task.key), "It wasn't restored. Try again.", (restored) => {
                  setChanged(restored);
                  setDeleted(false);
                })
              }
            />
          </View>
        </View>
      </View>
    );
  }

  const status = TASK_STATUS_META[task.status];
  const members = options?.members ?? [];
  const assigneeOptions = [
    { id: NOBODY, label: "Unassigned" },
    ...members.map((member) => ({ id: member.id, label: member.name })),
    // Keep the current assignee selectable even before members load.
    ...(task.assignee && !members.some((member) => member.id === task.assignee?.id)
      ? [{ id: task.assignee.id, label: task.assignee.name }]
      : []),
  ];
  // Projects with a shared key are the ones a task can belong to.
  const keyedProjects = (options?.projects ?? []).filter((project) => project.key !== null);
  const projectOptions = [
    { id: NO_PROJECT, label: "No project" },
    ...keyedProjects
      .filter((project, index) => keyedProjects.findIndex((other) => other.key === project.key) === index)
      .map((project) => ({ id: project.key!, label: project.label, hint: project.key! })),
    ...(task.projectKey && !keyedProjects.some((project) => project.key === task.projectKey)
      ? [{ id: task.projectKey, label: task.project ?? task.projectKey }]
      : []),
  ];
  const activity = fromServer?.activity ?? [];
  const labelSuggestions = (options?.labels ?? [])
    .filter((label) => !task.labels.includes(label) && label.includes(labelDraft.trim().toLowerCase()))
    .slice(0, 8);
  const addLabel = (label: string) => {
    const clean = label.trim().toLowerCase();
    if (!clean || task.labels.includes(clean) || task.labels.length >= 20) return;
    void change({ labels: [...task.labels, clean] }).then((ok) => ok && setLabelDraft(""));
  };

  // The task's workspace on this desktop, as the desktop's primary button
  // opens it.
  const workspaces = machine.catalog.status === "live" ? machine.catalog.catalog.workspaces : [];
  const workspace = [...task.links]
    .reverse()
    .filter((link) => link.kind === "workspace" && link.workspaceRoot)
    .map((link) => workspaces.find((entry) => entry.path === link.workspaceRoot))
    .find((entry) => entry !== undefined);
  const hasWorkspace = task.links.some((link) => link.kind === "workspace");

  // Start: the task's own project by default, else the first one.
  const agents = options?.agents ?? [];
  const startProjects = options?.projects ?? [];
  const defaultProject =
    startProjects.find((project) => project.key !== null && project.key === task.projectKey)?.path ??
    startProjects[0]?.path ??
    null;
  const chosenProject = startProject ?? defaultProject;
  const chosenAgent = startAgent ?? agents.find((agent) => agent.key !== "dragent")?.key ?? agents[0]?.key ?? null;
  const start = () => {
    if (!chosenAgent) return;
    setStartNote(null);
    void run(
      () => startTask(rpc!, { task: task.key, agent: chosenAgent, ...(chosenProject ? { project: chosenProject } : {}) }),
      "The desktop didn't answer. Check it's online, then try again.",
      (result) => {
        if (result.started) {
          setStartOpen(false);
          setStartNote("Started on the desktop: a new workspace is opening for this task. It shows under its project's workspaces.");
        } else {
          setStartNote(result.reason ?? "The desktop didn't start it.");
        }
      },
    );
  };

  return (
    <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
      {header}
      <ScrollScreen maxWidth={1040} contentStyle={styles.content}>
        <View style={styles.identityRow}>
          <StatusBadge label={status.label} tone={status.tone} />
          <AppText variant="caption" tone="textMuted" numberOfLines={1} style={styles.identity}>
            {[task.key, task.project, task.dueAt ? dueLabel(task.dueAt) : null].filter(Boolean).join(" · ")}
          </AppText>
        </View>
        {editing ? (
          <View style={styles.editor}>
            <FormField
              label="Title"
              value={editing.title}
              onChangeText={(title) => setEditing({ ...editing, title })}
              maxLength={500}
              autoFocus
            />
            <FormField
              label="Description"
              placeholder="Details, links, acceptance criteria…"
              value={editing.description}
              onChangeText={(description) => setEditing({ ...editing, description })}
              multiline
              maxLength={20_000}
            />
            {task.descriptionTruncated ? (
              <AppText variant="caption" tone="textMuted">
                This description was too long to bring to the phone whole; edit it on the desktop.
              </AppText>
            ) : null}
            <View style={styles.actions}>
              <Button label="Cancel" size="sm" variant="ghost" onPress={() => setEditing(null)} />
              <Button
                label="Save"
                size="sm"
                loading={busy}
                disabled={busy || !editing.title.trim()}
                onPress={() => {
                  const update: TaskChange = {};
                  if (editing.title.trim() !== task.title) update.title = editing.title;
                  if (!task.descriptionTruncated && editing.description !== task.description) update.description = editing.description;
                  if (Object.keys(update).length === 0) {
                    setEditing(null);
                    return;
                  }
                  void change(update).then((ok) => ok && setEditing(null));
                }}
              />
            </View>
          </View>
        ) : (
          <AppText variant="title" accessibilityRole="header" style={styles.title}>
            {task.title}
          </AppText>
        )}
        {task.creator ? (
          <View style={styles.byline}>
            <Avatar name={task.creator.name} size={16} />
            <AppText variant="caption" tone="textMuted" numberOfLines={1} style={styles.bylineText}>
              {`${task.creator.name} · updated ${shortAge(task.updatedAt)}`}
            </AppText>
          </View>
        ) : null}

        <OptionRow accessibilityLabel="Task details">
          <SelectPopover
            label="Status"
            caption="STATUS"
            options={TASK_STATUSES.map((key) => ({ id: key, label: TASK_STATUS_META[key].label }))}
            value={task.status}
            onChange={(next) => next !== task.status && void change({ status: next })}
            open={menu === "status"}
            onOpenChange={(open) => setMenu(open ? "status" : null)}
          />
          <SelectPopover
            label="Priority"
            caption="PRIORITY"
            options={TASK_PRIORITIES.map((key) => ({ id: key, label: TASK_PRIORITY_LABEL[key] }))}
            value={task.priority}
            onChange={(next) => next !== task.priority && void change({ priority: next })}
            open={menu === "priority"}
            onOpenChange={(open) => setMenu(open ? "priority" : null)}
          />
          <SelectPopover
            label="Assignee"
            caption="ASSIGN TO"
            options={assigneeOptions}
            value={task.assignee?.id ?? NOBODY}
            onChange={(next) => {
              const id = next === NOBODY ? "" : next;
              if (id !== (task.assignee?.id ?? "")) void change({ assigneeId: id });
            }}
            open={menu === "assignee"}
            onOpenChange={(open) => setMenu(open ? "assignee" : null)}
            searchable={assigneeOptions.length > 8}
            searchPlaceholder="Find a member"
          />
          <SelectPopover
            label="Project"
            caption="PROJECT"
            options={projectOptions}
            value={task.projectKey ?? NO_PROJECT}
            onChange={(next) => {
              const key = next === NO_PROJECT ? "" : next;
              if (key !== (task.projectKey ?? "")) void change({ projectKey: key });
            }}
            open={menu === "project"}
            onOpenChange={(open) => setMenu(open ? "project" : null)}
            triggerLabel={task.project ?? "No project"}
          />
          <Button
            label={task.dueAt ? dueLabel(task.dueAt) : "No due date"}
            accessibilityLabel={`Due date: ${task.dueAt ? dueLabel(task.dueAt) : "none"}. Change it`}
            size="sm"
            variant="ghost"
            leading={<Icon name="clock" size={14} />}
            onPress={() => setMenu(menu === "due" ? null : "due")}
          />
        </OptionRow>
        {menu === "due" ? (
          <DueDatePicker value={task.dueAt} disabled={busy} onPick={(dueAt) => void change({ dueAt })} />
        ) : null}
        {busy ? <AppText variant="caption" tone="textMuted">Saving…</AppText> : null}
        {copied ? <AppText variant="caption" tone="textMuted">{copied}</AppText> : null}
        <Feedback message={error} />

        {hasWorkspace ? (
          <Card title="Workspace" subtitle={workspace ? workspace.name : "This task's workspace isn't on this desktop."}>
            {workspace ? (
              <View style={styles.actions}>
                <Button label="Open workspace" size="sm" onPress={() => router.push(machineWorkspacePath(deviceId, workspaceKey(workspace)))} />
              </View>
            ) : null}
          </Card>
        ) : null}
        <Card title={hasWorkspace ? "Start again" : "Start work"} subtitle="An agent works on this task in a new workspace on the desktop.">
          {startNote ? <AppText tone="textMuted">{startNote}</AppText> : null}
          {startOpen ? (
            options === null ? (
              <LoadingState label="Loading the desktop's projects and agents…" />
            ) : startProjects.length === 0 ? (
              <AppText tone="textMuted">No projects are open on the desktop. Open one there, then start this task.</AppText>
            ) : (
              <View style={styles.editor}>
                <SelectPopover
                  label="Project"
                  caption="WORK IN"
                  options={startProjects.map((project) => ({ id: project.path, label: project.label, hint: project.path }))}
                  value={chosenProject ?? startProjects[0]!.path}
                  onChange={setStartProject}
                  open={menu === "start-project"}
                  onOpenChange={(open) => setMenu(open ? "start-project" : null)}
                  searchable={startProjects.length > 8}
                  searchPlaceholder="Find a project"
                />
                <AgentChips
                  label="Agent"
                  agents={agents}
                  selected={chosenAgent ? [chosenAgent] : []}
                  onChange={(next) => setStartAgent(next[0] ?? null)}
                  single
                  disabled={busy}
                />
                <View style={styles.actions}>
                  <Button label="Cancel" size="sm" variant="ghost" onPress={() => setStartOpen(false)} />
                  <Button label="Start" size="sm" loading={busy} disabled={busy || !chosenAgent} onPress={start} />
                </View>
              </View>
            )
          ) : (
            <View style={styles.actions}>
              <Button label="Start work" size="sm" disabled={rpc === null} onPress={() => setStartOpen(true)} />
            </View>
          )}
        </Card>

        <Card title="Description">
          {task.description.trim() ? (
            <AppText selectable>{task.description}</AppText>
          ) : (
            <AppText tone="textMuted">No description.</AppText>
          )}
          {task.descriptionTruncated ? (
            <AppText variant="caption" tone="textMuted">The rest is on the desktop.</AppText>
          ) : null}
        </Card>

        <Card title="Labels">
          {task.labels.length === 0 ? <AppText tone="textMuted">No labels.</AppText> : null}
          <View style={styles.chips}>
            {task.labels.map((label) => (
              <Control
                key={label}
                accessibilityRole="button"
                accessibilityLabel={`Remove label ${label}`}
                disabled={busy}
                onPress={() => void change({ labels: task.labels.filter((other) => other !== label) })}
                style={[styles.chip, { borderColor: theme.colors.border }]}
              >
                <AppText variant="caption">{label}</AppText>
                <Icon name="close" size={12} />
              </Control>
            ))}
          </View>
          <View style={styles.inline}>
            <FormField
              label="Add a label"
              placeholder="bug, design, …"
              value={labelDraft}
              onChangeText={setLabelDraft}
              maxLength={48}
              autoCapitalize="none"
              autoCorrect={false}
              containerStyle={styles.grow}
            />
            <Button label="Add" size="sm" variant="ghost" disabled={busy || !labelDraft.trim()} onPress={() => addLabel(labelDraft)} />
          </View>
          {labelSuggestions.length > 0 ? (
            <View style={styles.chips}>
              {labelSuggestions.map((label) => (
                <Control
                  key={label}
                  accessibilityRole="button"
                  accessibilityLabel={`Add label ${label}`}
                  disabled={busy}
                  onPress={() => addLabel(label)}
                  style={[styles.chip, { borderColor: theme.colors.border, borderStyle: "dashed" }]}
                >
                  <Icon name="plus" size={12} />
                  <AppText variant="caption" tone="textMuted">{label}</AppText>
                </Control>
              ))}
            </View>
          ) : null}
        </Card>

        <Card title="Links" subtitle="Pull requests, tracker items and workspaces for this task.">
          {task.links.length === 0 ? <AppText tone="textMuted">Nothing linked yet.</AppText> : null}
          {task.links.map((link) => (
            <LinkRow
              key={link.id}
              link={link}
              disabled={busy}
              onRemove={() => void run(() => unlinkFromTask(rpc!, task.key, link.id), "The link wasn't removed. Try again.", setChanged)}
            />
          ))}
          <View style={styles.inline}>
            <FormField
              label="Link a pull request or item"
              placeholder="https://github.com/… or linear:ENG-12"
              value={linkDraft}
              onChangeText={setLinkDraft}
              autoCapitalize="none"
              autoCorrect={false}
              maxLength={2048}
              containerStyle={styles.grow}
            />
            <Button
              label="Link"
              size="sm"
              variant="ghost"
              disabled={busy || !linkDraft.trim()}
              onPress={() =>
                void run(
                  () => linkToTask(rpc!, task.key, linkDraft),
                  "That wasn't linked. Use a pull request's web address, or a tracker item as PROVIDER:KEY (linear:ENG-12).",
                  (linked) => {
                    setChanged(linked);
                    setLinkDraft("");
                  },
                )
              }
            />
          </View>
        </Card>

        <Card title="Discussion" {...(comments.length ? { subtitle: `${comments.length} comment${comments.length === 1 ? "" : "s"}` } : {})}>
          {comments.length === 0 ? <AppText tone="textMuted">No comments yet.</AppText> : null}
          {comments.map((entry) => (
            <View key={entry.id} style={[styles.comment, { borderColor: theme.colors.border }]}>
              <View style={styles.byline}>
                <Avatar name={entry.author?.name ?? "Someone"} size={16} />
                <AppText variant="caption" tone="textMuted" numberOfLines={1} style={styles.bylineText}>
                  {`${entry.author?.name ?? "Someone"}${entry.byAgent ? " (agent)" : ""} · ${shortAge(entry.createdAt)}`}
                </AppText>
                {entry.mine && commentEdit?.id !== entry.id ? (
                  <View style={styles.commentActions}>
                    <IconButton name="pencil" label="Edit your comment" onPress={() => setCommentEdit({ id: entry.id, body: entry.body })} />
                    <IconButton
                      name="close"
                      label="Delete your comment"
                      onPress={() =>
                        void run(
                          () => deleteComment(rpc!, task.key, entry.id),
                          "Your comment wasn't deleted. Try again.",
                          () => setEdited((all) => ({ ...all, [entry.id]: null })),
                        )
                      }
                    />
                  </View>
                ) : null}
              </View>
              {commentEdit?.id === entry.id ? (
                <View style={styles.editor}>
                  <FormField
                    label="Edit comment"
                    value={commentEdit.body}
                    onChangeText={(body) => setCommentEdit({ id: entry.id, body })}
                    multiline
                    maxLength={20_000}
                    autoFocus
                  />
                  <View style={styles.actions}>
                    <Button label="Cancel" size="sm" variant="ghost" onPress={() => setCommentEdit(null)} />
                    <Button
                      label="Save"
                      size="sm"
                      disabled={busy || !commentEdit.body.trim()}
                      onPress={() =>
                        void run(
                          () => editComment(rpc!, task.key, entry.id, commentEdit.body),
                          "Your comment wasn't changed. Try again.",
                          (saved) => {
                            setEdited((all) => ({ ...all, [entry.id]: saved }));
                            setCommentEdit(null);
                          },
                        )
                      }
                    />
                  </View>
                </View>
              ) : (
                <AppText selectable>{entry.body}</AppText>
              )}
            </View>
          ))}
          <FormField
            label="Add a comment"
            placeholder="Write a comment…"
            value={draft}
            onChangeText={setDraft}
            multiline
            maxLength={20_000}
          />
          <View style={styles.actions}>
            <Button
              label="Comment"
              size="sm"
              disabled={!draft.trim() || busy}
              onPress={() =>
                void run(() => commentOnTask(rpc!, task.key, draft), "Your comment wasn't posted. Try again.", (added) => {
                  setPosted((list) => [...list, added]);
                  setDraft("");
                })
              }
            />
          </View>
        </Card>

        {activity.length > 0 ? (
          <Card title="History">
            {[...activity].reverse().slice(0, 30).map((entry) => (
              <AppText key={entry.id} variant="caption" tone="textMuted">
                {`${activityLine(entry)} · ${shortAge(entry.at)}`}
              </AppText>
            ))}
          </Card>
        ) : null}

        {confirmDelete ? (
          <ConfirmationPanel
            title={`Delete ${task.key}?`}
            message="It goes for everyone in your organisation. You can restore it right after."
            actionLabel="Delete"
            onCancel={() => setConfirmDelete(false)}
            onConfirm={() => {
              setConfirmDelete(false);
              void run(() => deleteTask(rpc!, task.key), "It wasn't deleted. Try again.", () => setDeleted(true));
            }}
          />
        ) : (
          <View style={styles.actions}>
            <Button label="Delete task" size="sm" variant="danger" disabled={busy || rpc === null} onPress={() => setConfirmDelete(true)} />
          </View>
        )}
      </ScrollScreen>
    </View>
  );
}

const LINK_KIND_LABEL: Record<TaskLink["kind"], string> = {
  pull_request: "Pull request",
  item: "Tracker item",
  workspace: "Workspace",
};

function LinkRow({ link, disabled, onRemove }: { link: TaskLink; disabled: boolean; onRemove: () => void }) {
  const { theme } = useTheme();
  const url = link.url;
  return (
    <View style={[styles.link, { borderColor: theme.colors.border }]}>
      <Icon name={link.kind === "pull_request" ? "pull-request" : link.kind === "workspace" ? "folder" : "link"} size={16} />
      <Control
        accessibilityRole={url ? "link" : "text"}
        accessibilityLabel={`${LINK_KIND_LABEL[link.kind]}: ${link.title}`}
        disabled={!url}
        onPress={() => url && void Linking.openURL(url)}
        style={styles.linkText}
      >
        <AppText numberOfLines={1}>{link.title}</AppText>
        <AppText variant="caption" tone="textMuted" numberOfLines={1}>
          {[LINK_KIND_LABEL[link.kind], link.state].filter(Boolean).join(" · ")}
        </AppText>
      </Control>
      {link.kind !== "workspace" ? (
        <IconButton name="close" label={`Remove ${link.title}`} disabled={disabled} onPress={onRemove} />
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create({
  frame: { flex: 1, minHeight: 0 },
  content: { gap: spacing.md },
  padded: { gap: 12, padding: 16 },
  headerActions: { flexDirection: "row", alignItems: "center" },
  identityRow: { flexDirection: "row", alignItems: "center", gap: spacing.sm, minWidth: 0 },
  identity: { flex: 1, minWidth: 0 },
  title: { fontWeight: "700" },
  editor: { gap: spacing.sm },
  byline: { flexDirection: "row", alignItems: "center", gap: 6, flexShrink: 1, minWidth: 0 },
  bylineText: { flexShrink: 1, minWidth: 0 },
  chips: { flexDirection: "row", flexWrap: "wrap", gap: spacing.xs },
  chip: {
    flexDirection: "row",
    alignItems: "center",
    gap: 4,
    minHeight: 32,
    paddingHorizontal: spacing.sm,
    borderRadius: 999,
    borderWidth: 1,
  },
  inline: { flexDirection: "row", alignItems: "flex-end", gap: spacing.sm },
  grow: { flex: 1, minWidth: 0 },
  comment: { gap: 4, paddingVertical: spacing.sm, borderBottomWidth: StyleSheet.hairlineWidth },
  commentActions: { flexDirection: "row", marginLeft: "auto" },
  link: {
    flexDirection: "row",
    alignItems: "center",
    gap: spacing.sm,
    minHeight: 44,
    paddingVertical: spacing.xs,
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
  linkText: { flex: 1, minWidth: 0 },
  actions: { flexDirection: "row", justifyContent: "flex-end", gap: spacing.sm },
});
