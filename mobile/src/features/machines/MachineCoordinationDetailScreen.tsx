import { useEffect, useState, type ReactNode } from "react";
import { StyleSheet, Switch, View } from "react-native";
import { router } from "expo-router";
import {
  AgentMark,
  AppText,
  Button,
  ConfirmationPanel,
  EmptyState,
  FormField,
  Icon,
  IconButton,
  KeyboardFrame,
  KeyboardScrollView,
  LoadingState,
  SegmentedControl,
  SelectPopover,
  StatusBadge,
  TabStrip,
  type IconName,
} from "../../components";
import { getAgentLabel, normalizeAgentKind } from "../../components/agentArtwork";
import { useResponsiveLayout } from "../../layout/useResponsiveLayout";
import { ago } from "./MachineCoordinationSection";
import { useTheme } from "../../theme";
import { radii, spacing } from "../../theme/tokens";
import { useMachine } from "../../machines/MachineContext";
import { MachineHeader } from "./MachineHeader";
import { Feedback } from "../shared/Feedback";
import { machinePath, machineTabPath } from "../../machines/routes";
import { projectLabelForPath } from "../../machines/machineCatalog";
import { useLiveCoordinationDetail } from "../../coordination/useLiveCoordination";
import {
  COORDINATION_MAX_PARALLEL,
  cancelCoordinationTask,
  cleanUpCoordination,
  configureCoordination,
  forgetCoordination,
  sendToLead,
  setCoordinationTaskModel,
  startNewLead,
  stopCoordination,
  type CoordinationWindowResult,
} from "../../coordination/rpc";
import {
  COORDINATION_COSTS,
  type CoordinationDetailResult,
  type CoordinationMessageKind,
  type CoordinationTaskStatus,
  type CoordinationTeam,
} from "../../coordination/types";

const FINISHED: readonly CoordinationTaskStatus[] = ["done", "failed", "cancelled"];
const AGENT_DEFAULT = "__agent_default__";
import {
  COORDINATION_STATUS_META,
  COORDINATION_TASK_META,
  CoordinationTruncatedNotice,
} from "../../coordination/CoordinationViews";

type DetailTab = "overview" | "agents" | "activity";

/**
 * Live coordination detail, with what the desktop's Coordination page does:
 * answer the team when it needs you, message its lead, open the lead's or a
 * worker's terminal, cancel a task, stop it, change how it runs (workers at
 * once, auto-start, whether the lead answers workers' prompts, cost), and
 * forget it once it's finished. The desktop checks and makes each change.
 * Ineligible moments show a meaningful hold with retry or a new session.
 */
export function MachineCoordinationDetailScreen({
  deviceId,
  teamId,
}: {
  deviceId: string;
  teamId: string;
}) {
  const { theme } = useTheme();
  const machine = useMachine();
  const live = useLiveCoordinationDetail(teamId);
  // The desktop's answer to a change here, shown until the next refresh.
  const [changed, setChanged] = useState<CoordinationDetailResult | null>(null);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [forgetting, setForgetting] = useState(false);
  const [cleaningUp, setCleaningUp] = useState(false);
  const [modelMenu, setModelMenu] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [tab, setTab] = useState<DetailTab>("overview");
  const layout = useResponsiveLayout();
  const [error, setError] = useState<string | null>(null);
  const liveResult = live.state.status === "live" ? live.state.result : null;
  useEffect(() => {
    setChanged(null);
  }, [liveResult]);
  const rpc = machine.rpc;
  const act = async (work: () => Promise<CoordinationDetailResult>, failed: string) => {
    if (rpc === null) return false;
    setBusy(true);
    setError(null);
    try {
      setChanged(await work());
      return true;
    } catch {
      setError(failed);
      return false;
    } finally {
      setBusy(false);
    }
  };
  /** A change a desktop window makes (new lead, clean up). */
  const windowAct = async (work: () => Promise<CoordinationWindowResult>, failed: string, doneNote: string) => {
    if (rpc === null) return;
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      const answer = await work();
      if (answer.done) {
        setChanged(answer.result);
        setNote(doneNote);
      } else {
        setError(answer.reason);
      }
    } catch {
      setError(failed);
    } finally {
      setBusy(false);
    }
  };
  const title =
    machine.catalog.status === "live"
      ? (machine.catalog.catalog.machine?.name ?? machine.catalog.machine.name)
      : "Machine";

  const back = machinePath(deviceId);
  const shownTeam = (changed ?? liveResult)?.team ?? null;
  const atWork = shownTeam !== null && (shownTeam.status === "planning" || shownTeam.status === "running");
  const header = (
    <MachineHeader
      title={shownTeam ? shownTeam.title : "Agent coordination"}
      subtitle={title}
      online={machine.catalog.status === "live" ? machine.catalog.machine.online : null}
      onBack={() =>
        router.navigate({ pathname: back.pathname, params: { ...back.params, view: "coordination" } })
      }
      backLabel="Back to Agent coordination"
      actions={
        shownTeam !== null && (shownTeam.lead_tab_id !== null || (atWork && rpc !== null)) ? (
          <View style={styles.headerActions}>
            {shownTeam.lead_tab_id !== null ? (
              <IconButton
                name="terminal"
                label="Open the lead's terminal"
                onPress={() => router.push(machineTabPath(deviceId, shownTeam.lead_tab_id!))}
              />
            ) : null}
            {atWork && rpc !== null ? (
              <IconButton name="stop" label="Stop this coordination" onPress={() => setStopping(true)} />
            ) : null}
          </View>
        ) : undefined
      }
    />
  );

  if (live.state.status === "loading") {
    return (
      <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
        {header}
        <LoadingState label="Loading live coordination…" />
      </View>
    );
  }

  if (live.state.status === "unavailable") {
    const ended = live.state.action === "reconnect";
    return (
      <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
        {header}
        <View style={styles.padded}>
          <Feedback message={live.state.reason} />
          <Button
            label={ended ? "Start new session" : "Retry"}
            size="sm"
            variant="ghost"
            onPress={ended ? live.reconnect : live.retry}
          />
        </View>
      </View>
    );
  }

  const { team, truncated } = changed ?? live.state.result;
  const status = COORDINATION_STATUS_META[team.status];
  const doneCount = team.tasks.filter((task) => task.status === "done").length;
  const share = team.tasks.length > 0 ? doneCount / team.tasks.length : 0;
  const project = projectLabelForPath(machine.catalog.status === "live" ? machine.catalog.catalog : null, team.project_path);
  const workers = team.tasks.filter((task) => task.agent !== null);
  const tabs = [
    { id: "overview" as const, label: "Overview", hint: "The plan and what needs you" },
    { id: "agents" as const, label: workers.length > 0 ? `Agents · ${workers.length + (team.lead_agent ? 1 : 0)}` : "Agents", hint: "The lead, its workers and how they run" },
    { id: "activity" as const, label: team.messages.length > 0 ? `Activity · ${team.messages.length}` : "Activity", hint: "Messages between you, the lead and its workers" },
  ];
  const composing = atWork && rpc !== null && !team.question;
  const gutter = { paddingHorizontal: layout.gutter };

  const taskActions = (task: CoordinationTeam["tasks"][number]) => {
    const cancellable = atWork && rpc !== null && !FINISHED.includes(task.status);
    if (task.worker_tab_id === null && !cancellable && !(task.can_set_model && rpc !== null && task.agent)) return null;
    return (
      <View style={styles.taskActions}>
        {task.can_set_model && rpc !== null && task.agent ? (
          <SelectPopover
            label={`Model for ${task.title}`}
            caption="MODEL"
            options={[
              { id: AGENT_DEFAULT, label: "Agent default" },
              ...(team.model_choices[task.agent] ?? []).map((model) => ({ id: model.id, label: model.label, hint: model.id })),
            ]}
            value={task.model_locked && task.chosen_model ? task.chosen_model : AGENT_DEFAULT}
            onChange={(next) => {
              const model = next === AGENT_DEFAULT ? null : next;
              void act(() => setCoordinationTaskModel(rpc, team.id, task.id, model), "The model didn't change. Try again.");
            }}
            open={modelMenu === task.id}
            onOpenChange={(open) => setModelMenu(open ? task.id : null)}
            searchable={(team.model_choices[task.agent]?.length ?? 0) > 8}
            searchPlaceholder="Find a model"
            triggerLabel={`Model: ${task.chosen_model ?? "Agent default"}${task.model_locked ? " · Your choice" : ""}`}
          />
        ) : null}
        {task.worker_tab_id !== null ? (
          <Button
            label="Terminal"
            size="sm"
            variant="ghost"
            leading={<Icon name="terminal" size={15} />}
            accessibilityLabel={`Open the terminal for ${task.title}`}
            onPress={() => router.push(machineTabPath(deviceId, task.worker_tab_id!))}
          />
        ) : null}
        {cancellable ? (
          <Button
            label="Cancel"
            size="sm"
            variant="ghost"
            accessibilityLabel={`Cancel ${task.title}`}
            disabled={busy}
            onPress={() => void act(() => cancelCoordinationTask(rpc!, team.id, task.id), "The task wasn't cancelled. Try again.")}
          />
        ) : null}
      </View>
    );
  };

  const overview = (
    <>
      {team.needs_new_lead && rpc !== null ? (
        <Panel tone="warning" icon="agent" title="The lead isn't running">
          <AppText>
            {team.tasks.length === 0
              ? "The lead stopped before it made a plan. A new lead starts again from your request."
              : "The lead isn't running, so nobody follows the workers or answers them. A new lead picks up from the plan and the messages."}
          </AppText>
          <View style={styles.rowEnd}>
            <Button
              label="Start a new lead"
              size="sm"
              loading={busy}
              disabled={busy}
              onPress={() =>
                void windowAct(
                  () => startNewLead(rpc, team.id),
                  "No new lead started. Check the desktop is online, then try again.",
                  "A new lead is starting on the desktop.",
                )
              }
            />
          </View>
        </Panel>
      ) : null}
      {team.question && rpc !== null ? (
        <Panel
          tone="accent"
          icon="message"
          title={team.question.kind === "needs_user" || team.question.kind === "question" ? `${team.question.label} asks you` : `From ${team.question.label}`}
        >
          <AppText selectable>{team.question.text}</AppText>
          <FormField
            label="Your answer"
            placeholder="Answer, decide, or say what to do…"
            value={draft}
            onChangeText={setDraft}
            multiline
            maxLength={8000}
          />
          <View style={styles.rowEnd}>
            <Button
              label="Answer"
              size="sm"
              loading={busy}
              disabled={!draft.trim() || busy}
              onPress={() => {
                void act(() => sendToLead(rpc, team.id, draft), "Your answer wasn't sent. Try again.").then((sent) => {
                  if (sent) setDraft("");
                });
              }}
            />
          </View>
        </Panel>
      ) : null}
      {team.outcome ? (
        <Panel tone="success" icon="check" title="Summary">
          <AppText selectable>{team.outcome}</AppText>
        </Panel>
      ) : null}
      {team.goal && team.goal !== team.title ? (
        <View style={styles.block}>
          <SectionLabel>Goal</SectionLabel>
          <AppText selectable>{team.goal}</AppText>
        </View>
      ) : null}
      <View style={styles.block}>
        <SectionLabel>{team.tasks.length > 0 ? `Plan · ${doneCount} of ${team.tasks.length} done` : "Plan"}</SectionLabel>
        {team.tasks.length === 0 ? (
          <EmptyState title="No plan yet" message="The lead is still planning." />
        ) : (
          team.tasks.map((task, index) => {
            const meta = COORDINATION_TASK_META[task.status];
            const after = task.depends_on.length > 0 ? `After ${task.depends_on.join(", ")}` : null;
            const note = task.result_summary ?? (task.reason && !FINISHED.includes(task.status) ? task.reason : null);
            return (
              <View key={task.id} style={[styles.taskCard, { borderColor: theme.colors.border, backgroundColor: theme.colors.surface }]}>
                <View style={styles.taskTop}>
                  <View style={[styles.step, { backgroundColor: theme.colors.surfaceRaised }]}>
                    <AppText variant="caption" style={styles.stepText}>{String(index + 1)}</AppText>
                  </View>
                  <AppText style={styles.taskTitle} numberOfLines={3}>{task.title}</AppText>
                  <StatusBadge label={meta.label} tone={meta.tone} />
                </View>
                {task.agent || after ? (
                  <View style={styles.taskMeta}>
                    {task.agent ? <AgentMark agent={task.agent} size={14} /> : null}
                    <AppText variant="caption" tone="textMuted" numberOfLines={1} style={styles.shrink}>
                      {[task.agent ? agentName(task.agent) : null, task.model, after].filter(Boolean).join(" · ")}
                    </AppText>
                  </View>
                ) : null}
                {note ? (
                  <AppText variant="caption" tone={task.status === "failed" || task.status === "blocked" ? "warning" : "textMuted"} numberOfLines={4}>
                    {note}
                  </AppText>
                ) : null}
                {taskActions(task)}
              </View>
            );
          })
        )}
      </View>
      {!atWork && rpc !== null ? (
        <View style={styles.block}>
          <SectionLabel>Finished</SectionLabel>
          {team.cleanup ? (
            cleaningUp ? (
              <ConfirmationPanel
                title="Clean up this coordination's worktrees?"
                message={[
                  `Closes and removes ${team.cleanup.remove.length} worktree${team.cleanup.remove.length === 1 ? "" : "s"}: ${team.cleanup.remove.join(", ")}. Their branches stay, with all the work.`,
                  team.cleanup.keep.length ? `Kept, to look at the result: ${team.cleanup.keep.join(", ")}.` : "",
                  team.cleanup.dirty.length ? `Left alone, with changes not committed: ${team.cleanup.dirty.join(", ")}.` : "",
                ]
                  .filter(Boolean)
                  .join(" ")}
                actionLabel="Clean up"
                onCancel={() => setCleaningUp(false)}
                onConfirm={() => {
                  setCleaningUp(false);
                  void windowAct(
                    () => cleanUpCoordination(rpc, team.id),
                    "Nothing was cleaned up. Check the desktop is online, then try again.",
                    "The worktrees are being removed on the desktop. Their branches stay.",
                  );
                }}
              />
            ) : null
          ) : null}
          {forgetting ? (
            <ConfirmationPanel
              title="Forget this coordination?"
              message="It comes off the list here and on the desktop. Its workspaces, branches and finished work stay."
              actionLabel="Forget"
              onCancel={() => setForgetting(false)}
              onConfirm={() => {
                setForgetting(false);
                setBusy(true);
                setError(null);
                forgetCoordination(rpc, team.id).then(
                  () => router.navigate({ pathname: back.pathname, params: { ...back.params, view: "coordination" } }),
                  () => {
                    setBusy(false);
                    setError("It wasn't forgotten. Try again.");
                  },
                );
              }}
            />
          ) : null}
          {!cleaningUp && !forgetting ? (
            <View style={styles.rowWrap}>
              {team.cleanup ? (
                <Button label="Clean up worktrees" size="sm" variant="ghost" disabled={busy} onPress={() => setCleaningUp(true)} />
              ) : null}
              <Button label="Forget coordination" size="sm" variant="danger" disabled={busy} onPress={() => setForgetting(true)} />
            </View>
          ) : null}
        </View>
      ) : null}
    </>
  );

  const leadState = team.needs_new_lead ? "Not running" : atWork ? "Running" : "Finished";
  const agents = (
    <>
      <View style={styles.block}>
        <SectionLabel>Lead</SectionLabel>
        <View style={[styles.agentCard, { borderColor: theme.colors.border, backgroundColor: theme.colors.surface }]}>
          {team.lead_agent ? <AgentMark agent={team.lead_agent} size={22} /> : <Icon name="agent" size={20} tone="text" />}
          <View style={styles.shrink}>
            <AppText style={styles.strong}>{team.lead_agent ? agentName(team.lead_agent) : "Lead"}</AppText>
            <AppText variant="caption" tone={team.needs_new_lead ? "warning" : "textMuted"}>{leadState} · plans and follows the work</AppText>
          </View>
          {team.lead_tab_id !== null ? (
            <IconButton name="terminal" label="Open the lead's terminal" onPress={() => router.push(machineTabPath(deviceId, team.lead_tab_id!))} />
          ) : null}
        </View>
      </View>
      <View style={styles.block}>
        <SectionLabel>{workers.length > 0 ? `Workers · ${workers.length}` : "Workers"}</SectionLabel>
        {workers.length === 0 ? (
          <AppText tone="textMuted">No worker has started yet.</AppText>
        ) : (
          workers.map((task) => {
            const meta = COORDINATION_TASK_META[task.status];
            return (
              <View key={task.id} style={[styles.agentCard, { borderColor: theme.colors.border, backgroundColor: theme.colors.surface }]}>
                <AgentMark agent={task.agent!} size={22} />
                <View style={styles.shrink}>
                  <AppText style={styles.strong} numberOfLines={1}>{agentName(task.agent!)}{task.model ? ` · ${task.model}` : ""}</AppText>
                  <AppText variant="caption" tone="textMuted" numberOfLines={2}>{task.title}</AppText>
                </View>
                <StatusBadge label={meta.label} tone={meta.tone} />
                {task.worker_tab_id !== null ? (
                  <IconButton name="terminal" label={`Open the terminal for ${task.title}`} onPress={() => router.push(machineTabPath(deviceId, task.worker_tab_id!))} />
                ) : null}
              </View>
            );
          })
        )}
      </View>
      <View style={styles.block}>
        <SectionLabel>Agents it may use</SectionLabel>
        <View style={styles.rowWrap}>
          {team.roster.length === 0 ? (
            <AppText tone="textMuted">Any installed agent</AppText>
          ) : (
            team.roster.map((agent) => (
              <View key={agent} style={[styles.chip, { backgroundColor: theme.colors.surfaceRaised }]}>
                <AgentMark agent={agent} size={14} />
                <AppText variant="caption">{agentName(agent)}</AppText>
              </View>
            ))
          )}
        </View>
      </View>
      {atWork && (team.max_parallel !== null || team.auto_start !== null || team.lead_answers_prompts !== null || team.cost !== null) ? (
        <View style={styles.block}>
          <SectionLabel>How it runs</SectionLabel>
          <View style={[styles.settings, { borderColor: theme.colors.border, backgroundColor: theme.colors.surface }]}>
            {team.max_parallel !== null ? (
              <SettingRow label="Workers at once" hint="Of one agent, running together">
                <View style={styles.stepper}>
                  <IconButton
                    name="minus"
                    label="Fewer workers at once"
                    disabled={busy || team.max_parallel <= 1}
                    onPress={() => void act(() => configureCoordination(rpc!, team.id, { maxParallel: team.max_parallel! - 1 }), "That change didn't go through.")}
                  />
                  <AppText style={styles.settingValue}>{String(team.max_parallel)}</AppText>
                  <IconButton
                    name="plus"
                    label="More workers at once"
                    disabled={busy || team.max_parallel >= COORDINATION_MAX_PARALLEL}
                    onPress={() => void act(() => configureCoordination(rpc!, team.id, { maxParallel: team.max_parallel! + 1 }), "That change didn't go through.")}
                  />
                </View>
              </SettingRow>
            ) : null}
            {team.auto_start !== null ? (
              <SettingRow label="Start next tasks on their own" hint={team.auto_start ? "On" : "Off: they wait for the lead"}>
                <Switch
                  accessibilityLabel="Start next tasks on their own"
                  value={team.auto_start}
                  disabled={busy}
                  onValueChange={(on) => void act(() => configureCoordination(rpc!, team.id, { autoStart: on }), "That change didn't go through.")}
                />
              </SettingRow>
            ) : null}
            {team.lead_answers_prompts !== null ? (
              <SettingRow label="Lead answers permission prompts" hint={team.lead_answers_prompts ? "On" : "Off: workers' prompts wait for you"}>
                <Switch
                  accessibilityLabel="Lead answers workers' permission prompts"
                  value={team.lead_answers_prompts}
                  disabled={busy}
                  onValueChange={(on) => void act(() => configureCoordination(rpc!, team.id, { leadAnswersPrompts: on }), "That change didn't go through.")}
                />
              </SettingRow>
            ) : null}
            {team.cost !== null ? (
              <View style={styles.costRow}>
                <AppText>Models</AppText>
                <SegmentedControl
                  label="Cost"
                  options={COORDINATION_COSTS}
                  value={team.cost}
                  size="sm"
                  onChange={(cost) => cost !== team.cost && void act(() => configureCoordination(rpc!, team.id, { cost }), "That change didn't go through.")}
                />
              </View>
            ) : null}
          </View>
        </View>
      ) : null}
    </>
  );

  const activity =
    team.messages.length === 0 ? (
      <EmptyState title="No activity yet" message="Messages between you, the lead and its workers appear here." />
    ) : (
      <View style={styles.messages}>
        {team.messages.map((message) => {
          const mine = message.from === "user";
          const asks = message.kind === "question" || message.kind === "needs_user";
          return (
            <View
              key={message.id}
              accessible
              accessibilityLabel={`${message.label}, ${message.kind}, ${new Date(message.created_at).toLocaleString()}: ${message.text}`}
              style={[
                styles.bubble,
                mine ? styles.bubbleMine : null,
                {
                  borderColor: asks ? theme.colors.warning : theme.colors.border,
                  backgroundColor: mine ? theme.colors.surfaceRaised : theme.colors.surface,
                },
              ]}
            >
              <View style={styles.bubbleHead}>
                <AppText variant="caption" style={styles.strong} numberOfLines={1}>{mine ? "You" : message.label}</AppText>
                {message.kind !== "note" ? <AppText variant="caption" tone={asks ? "warning" : "textMuted"}>{KIND_LABEL[message.kind]}</AppText> : null}
                <AppText variant="caption" tone="textSubtle" style={styles.time}>{ago(message.created_at)}</AppText>
              </View>
              <AppText selectable>{message.text}</AppText>
            </View>
          );
        })}
      </View>
    );

  return (
    <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
      {header}
      <View style={[styles.summary, gutter]}>
        <View style={styles.rowWrap}>
          <StatusBadge label={status.label} tone={status.tone} />
          <AppText variant="caption" tone="textMuted" numberOfLines={1} style={styles.shrink}>
            {`${project} · ${team.tasks.length > 0 ? `${doneCount} of ${team.tasks.length} done` : "planning"}`}
          </AppText>
        </View>
        {team.tasks.length > 0 ? (
          <View style={[styles.track, { backgroundColor: theme.colors.surfaceRaised }]}>
            <View style={[styles.trackFill, { width: `${share * 100}%`, backgroundColor: theme.colors.success }]} />
          </View>
        ) : null}
      </View>
      <TabStrip label="Coordination sections" options={tabs} value={tab} onChange={setTab} />
      <KeyboardFrame>
        <KeyboardScrollView style={styles.fill} contentContainerStyle={[styles.scroll, gutter]}>
          <View style={styles.content}>
            {truncated ? <CoordinationTruncatedNotice label="Showing limited recent activity for tasks and messages." /> : null}
            {stopping ? (
              <ConfirmationPanel
                title="Stop this coordination?"
                message="The lead and every worker stop on the desktop. Finished work stays; unfinished tasks don't continue."
                actionLabel="Stop"
                onCancel={() => setStopping(false)}
                onConfirm={() => {
                  setStopping(false);
                  void act(() => stopCoordination(rpc!, team.id), "It wasn't stopped. Check the desktop is online, then try again.");
                }}
              />
            ) : null}
            <Feedback message={error} />
            {note ? <AppText tone="textMuted">{note}</AppText> : null}
            {tab === "overview" ? overview : tab === "agents" ? agents : activity}
          </View>
        </KeyboardScrollView>
        {composing ? (
          <View style={[styles.composer, gutter, { borderColor: theme.colors.border, backgroundColor: theme.colors.surface }]}>
            <View style={[styles.composerRow, { maxWidth: 1040 }]}>
              <View style={styles.shrink}>
                <FormField
                  label="Message the lead"
                  placeholder={team.needs_new_lead ? "No lead is running: the next lead reads it" : "Ask the lead or change the plan…"}
                  value={draft}
                  onChangeText={setDraft}
                  multiline
                  maxLength={8000}
                />
              </View>
              <IconButton
                name="arrow-up"
                label="Send to lead"
                disabled={!draft.trim() || busy}
                onPress={() => {
                  void act(() => sendToLead(rpc, team.id, draft), "Your message wasn't sent. Try again.").then((sent) => {
                    if (sent) setDraft("");
                  });
                }}
              />
            </View>
          </View>
        ) : null}
      </KeyboardFrame>
    </View>
  );
}

const KIND_LABEL: Record<CoordinationMessageKind, string> = {
  note: "Note",
  question: "Question",
  answer: "Answer",
  handoff: "Handoff",
  finding: "Finding",
  needs_user: "Needs you",
};

function agentName(agent: string): string {
  const kind = normalizeAgentKind(agent);
  return kind ? getAgentLabel(kind) : agent;
}

function SectionLabel({ children }: { children: string }) {
  return (
    <AppText variant="micro" tone="textMuted" style={styles.sectionLabel}>
      {children.toUpperCase()}
    </AppText>
  );
}

function SettingRow({ label, hint, children }: { label: string; hint: string; children: ReactNode }) {
  return (
    <View style={styles.settingRow}>
      <View style={styles.shrink}>
        <AppText>{label}</AppText>
        <AppText variant="caption" tone="textMuted">{hint}</AppText>
      </View>
      {children}
    </View>
  );
}

function Panel({ tone, icon, title, children }: { tone: "accent" | "warning" | "success"; icon: IconName; title: string; children: ReactNode }) {
  const { theme } = useTheme();
  const color = tone === "accent" ? theme.colors.accent : theme.colors[tone];
  return (
    <View style={[styles.panel, { borderColor: color, backgroundColor: theme.colors.surface }]}>
      <View style={styles.panelHead}>
        <Icon name={icon} size={15} tone={tone === "accent" ? "accent" : tone} />
        <AppText style={styles.strong}>{title}</AppText>
      </View>
      {children}
    </View>
  );
}

const styles = StyleSheet.create({
  frame: { flex: 1 },
  fill: { flex: 1, minHeight: 0 },
  padded: { gap: 12, padding: 16 },
  headerActions: { flexDirection: "row", alignItems: "center" },
  summary: { gap: spacing.sm, paddingVertical: spacing.md },
  track: { height: 4, borderRadius: 2, overflow: "hidden" },
  trackFill: { height: 4, borderRadius: 2 },
  scroll: { flexGrow: 1, alignItems: "center", paddingVertical: spacing.lg },
  content: { width: "100%", maxWidth: 1040, gap: spacing.lg },
  block: { gap: spacing.sm },
  sectionLabel: { paddingTop: spacing.xs },
  rowWrap: { flexDirection: "row", flexWrap: "wrap", alignItems: "center", gap: spacing.sm },
  rowEnd: { flexDirection: "row", justifyContent: "flex-end" },
  shrink: { flex: 1, minWidth: 0 },
  strong: { fontWeight: "600" },
  panel: { borderWidth: 1, borderRadius: radii.xl, borderCurve: "continuous", padding: spacing.md, gap: spacing.sm },
  panelHead: { flexDirection: "row", alignItems: "center", gap: spacing.sm },
  taskCard: { borderWidth: 1, borderRadius: radii.xl, borderCurve: "continuous", padding: spacing.md, gap: spacing.sm },
  taskTop: { flexDirection: "row", alignItems: "flex-start", gap: spacing.sm },
  step: { width: 22, height: 22, borderRadius: 11, alignItems: "center", justifyContent: "center", marginTop: 1 },
  stepText: { fontWeight: "600" },
  taskTitle: { flex: 1, minWidth: 0, fontWeight: "600" },
  taskMeta: { flexDirection: "row", alignItems: "center", gap: spacing.xs, paddingLeft: 30 },
  taskActions: { flexDirection: "row", flexWrap: "wrap", alignItems: "center", justifyContent: "flex-end", gap: spacing.xs },
  agentCard: { flexDirection: "row", alignItems: "center", gap: spacing.md, borderWidth: 1, borderRadius: radii.xl, borderCurve: "continuous", paddingVertical: spacing.sm, paddingHorizontal: spacing.md, minHeight: 56 },
  chip: { flexDirection: "row", alignItems: "center", gap: spacing.xs, borderRadius: radii.md, paddingHorizontal: spacing.sm, paddingVertical: spacing.xs },
  settings: { borderWidth: 1, borderRadius: radii.xl, borderCurve: "continuous", paddingHorizontal: spacing.md, paddingVertical: spacing.xs },
  settingRow: { flexDirection: "row", alignItems: "center", gap: spacing.md, minHeight: 56, paddingVertical: spacing.xs },
  stepper: { flexDirection: "row", alignItems: "center", gap: spacing.xs },
  settingValue: { minWidth: 24, textAlign: "center", fontWeight: "600" },
  costRow: { gap: spacing.sm, paddingVertical: spacing.sm },
  messages: { gap: spacing.sm },
  bubble: { borderWidth: 1, borderRadius: radii.xl, borderCurve: "continuous", padding: spacing.md, gap: spacing.xs, maxWidth: "92%", alignSelf: "flex-start" },
  bubbleMine: { alignSelf: "flex-end" },
  bubbleHead: { flexDirection: "row", alignItems: "center", gap: spacing.sm },
  time: { marginLeft: "auto" },
  composer: { borderTopWidth: 1, paddingVertical: spacing.sm, alignItems: "center" },
  composerRow: { width: "100%", flexDirection: "row", alignItems: "flex-end", gap: spacing.sm },
});
