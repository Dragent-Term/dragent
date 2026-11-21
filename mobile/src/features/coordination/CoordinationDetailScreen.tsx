import { useMemo, useState, type ReactNode } from "react";
import { StyleSheet, View } from "react-native";
import { router } from "expo-router";
import {
  AppText,
  Button,
  EmptyState,
  FormField,
  Icon,
  IconButton,
  StatusBadge,
  TabStrip,
  ConfirmationPanel,
  type StatusTone,
} from "../../components";
import { Screen, ScrollScreen } from "../../layout";
import { useTheme } from "../../theme";
import { spacing } from "../../theme/tokens";
import { usePreview } from "../../preview/PreviewContext";
import { PreviewHeader } from "../../preview/PreviewHeader";
import { PREVIEW_LIMITS } from "../../preview/PreviewStore";
import { selectCoordination, selectMachineTab } from "../../preview/machineSelectors";
import type { PreviewCoordination } from "../../preview/machineTypes";
import type { PreviewTab } from "../../preview/types";
import { Feedback } from "../shared/Feedback";
import { COORDINATION_STATUS } from "./CoordinationArea";
import { CoordinationMessageRow } from "../../coordination/CoordinationViews";

type DetailView = "overview" | "activity";

const VIEW_OPTIONS = [
  { id: "overview", label: "Overview", hint: "Plan and facts" },
  { id: "activity", label: "Activity", hint: "Messages with the lead" },
] as const;

const PLAN_STATUS: Record<PreviewCoordination["plan"][number]["status"], { label: string; tone: StatusTone }> = {
  pending: { label: "Pending", tone: "neutral" },
  working: { label: "Working", tone: "working" },
  done: { label: "Done", tone: "success" },
  blocked: { label: "Blocked", tone: "warning" },
  cancelled: { label: "Cancelled", tone: "neutral" },
};

const FROM_LABEL: Record<PreviewCoordination["messages"][number]["from"], string> = {
  lead: "Lead",
  you: "You",
  system: "System",
};

const MAX_PARALLEL = PREVIEW_LIMITS.parallelWorkers;

function planState(task: PreviewCoordination["plan"][number]): string {
  if (task.status === "pending") {
    return task.dependsOn.length > 0 ? "waiting" : "ready to start";
  }
  if (task.status === "working") return "working";
  if (task.status === "done") return "done";
  if (task.status === "blocked") return "blocked";
  return "cancelled";
}

/** One plan task: title, status word, dependency meta, and its worker tab when set. */
function PlanTaskRow({
  task,
  worker,
  onOpenWorker,
}: {
  task: PreviewCoordination["plan"][number];
  worker: PreviewTab | null;
  onOpenWorker: (tabId: string) => void;
}) {
  const { theme } = useTheme();
  const status = PLAN_STATUS[task.status];
  const meta = task.dependsOn.length > 0 ? `after ${task.dependsOn.join(", ")}` : null;
  // Plain container, not an accessible group: grouping would hide the nested
  // worker button from native screen readers. Title, meta, and status stay
  // individually readable; the button keeps its own label.
  return (
    <View style={[styles.taskRow, { borderColor: theme.colors.border }]}>
      <View style={[styles.icon, { backgroundColor: theme.colors.surfaceRaised, borderColor: theme.colors.border }]}>
        <Icon name="tasks" size={18} tone="text" />
      </View>
      <View style={styles.copy}>
        <AppText style={styles.name} numberOfLines={3}>{task.title}</AppText>
        <AppText variant="caption" tone="textMuted" numberOfLines={2}>
          {meta ? `${planState(task)} · ${meta}` : planState(task)}
        </AppText>
        {worker ? (
          <AppText variant="caption" tone="textMuted" numberOfLines={1}>{worker.title}</AppText>
        ) : null}
      </View>
      <View style={styles.trailing}>
        <StatusBadge label={status.label} tone={status.tone} />
        {worker ? (
          <IconButton name="terminal" label={`Open worker tab for ${task.title}`} onPress={() => onOpenWorker(worker.id)} />
        ) : null}
      </View>
    </View>
  );
}

/** One facts row: fixed label plus its value control. */
function FactRow({ label, children }: { label: string; children: ReactNode }) {
  const { theme } = useTheme();
  return (
    <View style={[styles.factRow, { borderColor: theme.colors.border }]}>
      <AppText variant="caption" tone="textMuted" style={styles.factLabel}>{label}</AppText>
      <View style={styles.factValue}>{children}</View>
    </View>
  );
}

/**
 * One coordination: goal, lead, plan with dependencies/workers, facts
 * (parallelism/autostart), message composer, and Stop. Strict desktop
 * ownership via the shared selector; linked tabs open the canonical leaf.
 */
export function CoordinationDetailScreen({
  desktopId,
  coordinationId,
}: {
  desktopId: string;
  coordinationId: string;
}) {
  const { theme } = useTheme();
  const preview = usePreview();
  const [view, setView] = useState<DetailView>("overview");
  const [stopping, setStopping] = useState(false);
  const [draft, setDraft] = useState("");
  const [feedback, setFeedback] = useState<string | null>(null);

  const desktop = preview.desktops.find((entry) => entry.id === desktopId) ?? null;
  const coordination = selectCoordination(preview, desktopId, coordinationId);
  const project = coordination
    ? (preview.projects.find((entry) => entry.id === coordination.projectId) ?? null)
    : null;
  const leadTab = coordination ? selectMachineTab(preview, desktopId, coordination.leadTabId) : null;

  const workers = useMemo(() => {
    const out = new Map<string, PreviewTab>();
    if (!coordination) return out;
    for (const task of coordination.plan) {
      if (!task.tabId || out.has(task.tabId)) continue;
      const worker = selectMachineTab(preview, desktopId, task.tabId);
      if (worker) out.set(task.tabId, worker);
    }
    return out;
  }, [preview, desktopId, coordination]);

  const backToHub = () =>
    router.navigate({ pathname: "/preview/desktop/[desktopId]", params: { desktopId, view: "coordination" } });

  const openTab = (tabId: string) => {
    const tab = selectMachineTab(preview, desktopId, tabId);
    if (!tab || !coordination) return;
    router.push({
      pathname: "/preview/desktop/[desktopId]/project/[projectId]/workspace/[workspaceId]/tab/[tabId]",
      params: { desktopId, projectId: tab.projectId, workspaceId: tab.workspaceId, tabId: tab.id },
    });
  };

  if (!desktop || !coordination) {
    return (
      <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
        <PreviewHeader
          title="Coordination unavailable"
          onBack={() => router.replace(desktop ? { pathname: "/preview/desktop/[desktopId]", params: { desktopId: desktop.id } } : "/preview")}
          backLabel={desktop ? `Back to ${desktop.name}` : "Back to desktops"}
        />
        <Screen maxWidth={1040} contentStyle={styles.content}>
          <EmptyState
            title="Coordination not found"
            message="This sample coordination is not part of that machine."
            actionLabel="Back to desktops"
            onAction={() => router.replace("/preview")}
          />
        </Screen>
      </View>
    );
  }

  const status = COORDINATION_STATUS[coordination.status];
  const atWork = coordination.status === "planning" || coordination.status === "running";
  const doneCount = coordination.plan.filter((task) => task.status === "done").length;

  const send = () => {
    const text = draft.trim();
    if (!text) return;
    if (preview.sendCoordinationMessage(desktopId, coordination.id, text)) {
      setDraft("");
      setFeedback(null);
    } else {
      setFeedback("Could not send. Nothing leaves this preview.");
    }
  };

  return (
    <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
      <PreviewHeader
        title={coordination.title}
        subtitle={`${project?.name ?? "Unknown project"} · ${status.label}`}
        onBack={backToHub}
        backLabel="Back to Agent coordination"
        actions={
          atWork ? <IconButton name="stop" label="Stop coordination" onPress={() => setStopping(true)} /> : undefined
        }
      />
      {stopping && atWork ? <ConfirmationPanel title="Stop coordination?"
        message="Unfinished tasks are cancelled. Completed work and the agents' tabs stay available."
        actionLabel="Stop coordination locally" cancelLabel="Keep going"
        onCancel={() => setStopping(false)} onConfirm={() => {
          setStopping(false);
          if (!preview.stopCoordination(desktopId, coordination.id)) setFeedback("Could not stop this sample coordination.");
        }} /> : null}
      <TabStrip label="Coordination view" options={VIEW_OPTIONS} value={view} onChange={setView} />
      <ScrollScreen maxWidth={1040} contentStyle={styles.content}>
        <AppText variant="caption" tone="textSubtle">
          Sample coordination · changes stay in this preview.
        </AppText>
        {view === "overview" ? (
          <View style={styles.section}>
            <AppText style={styles.goal}>{coordination.goal}</AppText>
            <View style={[styles.leadRow, { borderColor: theme.colors.border }]}>
              <View style={[styles.icon, { backgroundColor: theme.colors.surfaceRaised, borderColor: theme.colors.border }]}>
                <Icon name="agent" size={18} tone="text" />
              </View>
              <View style={styles.copy}>
                <AppText style={styles.name} numberOfLines={2}>Lead agent</AppText>
                <AppText variant="caption" tone="textMuted" numberOfLines={2}>
                  {leadTab ? leadTab.title : "Lead tab unavailable"}
                </AppText>
              </View>
              {leadTab ? (
                <Button label="Open lead tab" size="sm" variant="ghost" onPress={() => openTab(leadTab.id)} />
              ) : null}
            </View>
            <AppText variant="caption" tone="textMuted">
              {`Facts · ${doneCount} of ${coordination.plan.length} done`}
            </AppText>
            <View>
              <FactRow label="At once">
                <View style={styles.stepper}>
                  <Button label="−" size="sm" variant="ghost" accessibilityLabel="Fewer parallel workers" disabled={!atWork || coordination.parallelism <= 1} onPress={() => { preview.configureCoordination(desktopId, coordination.id, { parallelism: coordination.parallelism - 1 }); }} />
                  <AppText numberOfLines={1}>{`${coordination.parallelism} per agent`}</AppText>
                  <Button label="+" size="sm" variant="ghost" accessibilityLabel="More parallel workers" disabled={!atWork || coordination.parallelism >= MAX_PARALLEL} onPress={() => { preview.configureCoordination(desktopId, coordination.id, { parallelism: coordination.parallelism + 1 }); }} />
                </View>
              </FactRow>
              <FactRow label="Next tasks">
                <View style={styles.stepper}>
                  <AppText numberOfLines={1}>{coordination.autoStart ? "Start on their own" : "Wait for the lead"}</AppText>
                  <Button
                    label={coordination.autoStart ? "Turn off" : "Turn on"}
                    size="sm"
                    variant="ghost"
                    disabled={!atWork}
                    onPress={() => { preview.configureCoordination(desktopId, coordination.id, { autoStart: !coordination.autoStart }); }}
                  />
                </View>
              </FactRow>
            </View>
            <AppText variant="caption" tone="textMuted">Plan</AppText>
            {coordination.plan.length === 0 ? (
              <EmptyState title="No plan yet" message="The lead is still planning." />
            ) : (
              <View>
                {coordination.plan.map((task) => (
                  <PlanTaskRow
                    key={task.id}
                    task={task}
                    worker={task.tabId ? (workers.get(task.tabId) ?? null) : null}
                    onOpenWorker={openTab}
                  />
                ))}
              </View>
            )}
          </View>
        ) : (
          <View style={styles.section}>
            {coordination.messages.length === 0 ? (
              <EmptyState title="No activity yet" message="Messages with the lead appear here." />
            ) : (
              <View>
              {coordination.messages.map((message) => (
                <CoordinationMessageRow
                  key={message.id}
                  message={{
                    key: message.id,
                    header: `${FROM_LABEL[message.from]}, ${new Date(message.at).toLocaleString()}`,
                    body: message.body,
                    accessibilityLabel: `${FROM_LABEL[message.from]}, ${new Date(message.at).toLocaleString()}: ${message.body}`,
                  }}
                />
              ))}
              </View>
            )}
            <FormField
              label="Message the lead"
              value={draft}
              onChangeText={(text) => { setDraft(text); }}
              autoCapitalize="sentences"
              returnKeyType="send"
              placeholder="Ask the lead…"
              hint="Your message is added to this sample activity."
              onSubmitEditing={() => { if (draft.trim()) send(); }}
            />
            <View style={styles.actions}>
              <Button label="Send to lead" disabled={!draft.trim()} onPress={send} />
            </View>
            <Feedback message={feedback} />
          </View>
        )}
      </ScrollScreen>
    </View>
  );
}

const styles = StyleSheet.create({
  frame: { flex: 1 },
  content: { gap: spacing.md },
  section: { gap: spacing.md },
  goal: { fontWeight: "600" },
  leadRow: {
    borderBottomWidth: 1,
    flexDirection: "row",
    flexWrap: "wrap",
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
  copy: { flexGrow: 1, flexShrink: 1, flexBasis: 160, minWidth: 0, gap: 2 },
  trailing: { flexDirection: "row", alignItems: "center", gap: spacing.sm, marginLeft: "auto" },
  name: { fontWeight: "600" },
  factRow: {
    borderBottomWidth: 1,
    flexDirection: "row",
    alignItems: "center",
    gap: spacing.md,
    paddingVertical: spacing.sm,
    paddingHorizontal: spacing.xs,
  },
  factLabel: { width: 84, flexShrink: 0 },
  factValue: { flex: 1, minWidth: 0 },
  stepper: { flexDirection: "row", flexWrap: "wrap", alignItems: "center", gap: spacing.sm },
  taskRow: {
    borderBottomWidth: 1,
    flexDirection: "row",
    flexWrap: "wrap",
    alignItems: "center",
    gap: spacing.md,
    paddingVertical: spacing.sm,
    paddingHorizontal: spacing.xs,
  },
  messageRow: { borderBottomWidth: 1, gap: 2, paddingVertical: spacing.sm, paddingHorizontal: spacing.xs },
  actions: { flexDirection: "row", flexWrap: "wrap", gap: spacing.sm },
});
