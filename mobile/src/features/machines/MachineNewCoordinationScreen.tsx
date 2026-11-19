import { useState } from "react";
import { StyleSheet, View } from "react-native";
import { router } from "expo-router";
import { AgentMark, AppText, Button, Control, Icon, LoadingState, SelectPopover } from "../../components";
import { ScrollScreen } from "../../layout";
import { useTheme } from "../../theme";
import { spacing } from "../../theme/tokens";
import { useMachine } from "../../machines/MachineContext";
import { machinePath } from "../../machines/routes";
import { MachineHeader } from "./MachineHeader";
import { Feedback } from "../shared/Feedback";
import { useStartOptions } from "./StartPickers";
import { ChatComposer, ComposerButton } from "../chat/ChatParts";
import { ChatAttachmentStrip, ChatExtrasBar, ChatExtrasTray, useChatExtras } from "../chat/ChatExtras";
import { IssuesSheet, LinkedIssueChips, WorkersSheet } from "./CoordinationSheets";
import { MAX_GOAL_CHARS, goalWithLinks, type LinkedIssue } from "../../coordination/goal";
import { coordinationOptions, createCoordination } from "../../coordination/rpc";
import { COORDINATION_COSTS, type CoordinationCost } from "../../coordination/types";

/**
 * A new coordination, started on the paired desktop the way its prompt's
 * Coordinate does: the lead agent plans the goal in a project and hands
 * the parts to the worker agents, spending as the cost setting says. The
 * goal is written in the chat's message box: dictate it, attach photos and
 * files, link issues from the desktop's trackers.
 */
export function MachineNewCoordinationScreen({ deviceId }: { deviceId: string }) {
  const { theme } = useTheme();
  const machine = useMachine();
  const rpc = machine.rpc;
  const { options, error: loadError, reload } = useStartOptions(rpc, coordinationOptions);
  const [project, setProject] = useState<string | null>(null);
  const [goal, setGoal] = useState("");
  const [lead, setLead] = useState<string | null>(null);
  const [workers, setWorkers] = useState<string[]>([]);
  const [cost, setCost] = useState<CoordinationCost>("balanced");
  const [menu, setMenu] = useState<"project" | "lead" | "cost" | null>(null);
  const [sheet, setSheet] = useState<"workers" | "issues" | null>(null);
  const [issues, setIssues] = useState<LinkedIssue[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [started, setStarted] = useState(false);
  const projectRoot = project ?? options?.projects[0]?.path ?? null;
  // Photos and files go to the desktop; `@` finds files in the project.
  const extras = useChatExtras({ tabId: null, workspaceRoot: projectRoot });

  const back = machinePath(deviceId);
  const toList = () => router.navigate({ pathname: back.pathname, params: { ...back.params, view: "coordination" } });
  const header = (
    <MachineHeader
      title="New coordination"
      subtitle="A lead agent and its workers"
      online={machine.catalog.status === "live" ? machine.catalog.machine.online : null}
      onBack={toList}
      backLabel="Back to Agent coordination"
    />
  );

  if (options === null) {
    return (
      <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
        {header}
        {loadError ? (
          <View style={styles.padded}>
            <Feedback message={loadError} />
            <Button label="Retry" size="sm" variant="ghost" onPress={reload} />
          </View>
        ) : (
          <LoadingState label="Loading the desktop's projects and agents…" />
        )}
      </View>
    );
  }

  const projects = options.projects;
  const chosenProject = project ?? projects[0]?.path ?? null;
  // A terminal agent leads by default: Dragent's own can't lead on SSH machines.
  const chosenLead = lead ?? options.agents.find((agent) => agent.key !== "dragent")?.key ?? options.agents[0]?.key ?? null;
  // Dragent's own agent leads but doesn't take tasks.
  const workerChoices = options.agents.filter((agent) => agent.key !== "dragent");

  const files = extras.attachments.filter((item) => item.path).map((item) => item.path!);
  const fullGoal = goalWithLinks(goal, issues, files);
  const ready = rpc !== null && !!chosenProject && !!chosenLead && !extras.uploading && fullGoal.trim() !== "";

  const start = async () => {
    if (!ready || rpc === null || !chosenProject || !chosenLead) return;
    if (fullGoal.length > MAX_GOAL_CHARS) {
      setError("The goal and its links are too long. Shorten the goal or link fewer issues.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const result = await createCoordination(rpc, { project: chosenProject, goal: fullGoal, lead: chosenLead, workers, cost });
      if (result.started) setStarted(true);
      else setError(result.reason ?? "The desktop didn't start it.");
    } catch {
      setError("The desktop didn't answer. Check it's online, then try again.");
    } finally {
      setBusy(false);
    }
  };

  if (started) {
    return (
      <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
        {header}
        <View style={styles.padded}>
          <AppText variant="title">Started on the desktop</AppText>
          <AppText tone="textMuted">
            The lead is opening in a new workspace and will plan the work. The coordination appears in the list as soon as it has a plan.
          </AppText>
          <View style={styles.actions}>
            <Button label="Back to Agent coordination" size="sm" onPress={toList} />
          </View>
        </View>
      </View>
    );
  }

  const workerLabels = workers
    .map((key) => workerChoices.find((agent) => agent.key === key))
    .filter((agent): agent is NonNullable<typeof agent> => agent !== undefined);
  const workerSummary = workerLabels.length === 0
    ? "Only the lead's kind"
    : workerLabels.length <= 2
      ? workerLabels.map((agent) => agent.label).join(", ")
      : `${workerLabels[0]!.label} +${workerLabels.length - 1}`;
  const hairline = <View style={[styles.hairline, { backgroundColor: theme.colors.border }]} />;

  return (
    <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
      {header}
      <ScrollScreen maxWidth={720} contentStyle={styles.content}>
        <View style={styles.intro}>
          <AppText variant="title">What should the team get done?</AppText>
          <AppText variant="caption" tone="textMuted">The lead plans it and hands the parts to its workers.</AppText>
        </View>

        <View style={styles.composer}>
          <ChatComposer
            floating={false}
            value={goal}
            onChangeText={(text) => setGoal(text.slice(0, MAX_GOAL_CHARS))}
            placeholder="Describe the goal, or link an issue…"
            accessibilityLabel="Goal"
            accessibilityHint="Send starts the coordination on the desktop."
            onSend={() => void start()}
            sending={busy}
            canSend={ready}
            error={extras.notice}
            hasContent={issues.length > 0 || extras.hasAttachments}
            top={<ChatExtrasTray extras={extras} draft={goal} onDraft={setGoal} />}
            inside={
              <>
                <LinkedIssueChips issues={issues} onRemove={(ref) => setIssues((current) => current.filter((issue) => issue.ref !== ref))} />
                <ChatAttachmentStrip extras={extras} />
              </>
            }
            bottom={
              <ChatExtrasBar extras={extras}>
                <ComposerButton icon="link" label="Link issues" onPress={() => { extras.setMenuOpen(false); setSheet("issues"); }} />
              </ChatExtrasBar>
            }
          />
        </View>
        <Feedback message={error} />

        <View style={styles.section}>
          <AppText variant="micro" tone="textSubtle" style={styles.sectionCaption}>THE TEAM</AppText>
          <View style={[styles.card, { backgroundColor: theme.colors.surface, borderColor: theme.colors.border }]}>
            {projects.length === 0 ? (
              <AppText tone="textMuted" style={styles.cardNote}>No projects are open on the desktop. Open one there first.</AppText>
            ) : (
              <SelectPopover
                row={{ icon: "folder" }}
                label="Project"
                caption="PROJECT"
                options={projects.map((entry) => ({ id: entry.path, label: entry.label, hint: entry.path }))}
                value={chosenProject ?? projects[0]!.path}
                onChange={setProject}
                open={menu === "project"}
                onOpenChange={(open) => setMenu(open ? "project" : null)}
                searchable={projects.length > 8}
                searchPlaceholder="Find a project"
              />
            )}
            {hairline}
            {chosenLead ? (
              <SelectPopover
                row={{ icon: "agent" }}
                label="Lead"
                caption="PLANS AND HANDS OUT THE WORK"
                options={options.agents.map((agent) => ({ id: agent.key, label: agent.label, leading: <AgentMark agent={agent.key} size={16} /> }))}
                value={chosenLead}
                onChange={setLead}
                open={menu === "lead"}
                onOpenChange={(open) => setMenu(open ? "lead" : null)}
                searchable={options.agents.length > 8}
                searchPlaceholder="Find an agent"
              />
            ) : null}
            {hairline}
            <Control
              accessibilityRole="button"
              accessibilityLabel={`Workers: ${workerSummary}`}
              onPress={() => setSheet("workers")}
              style={({ pressed, hovered }) => [styles.rowTrigger, pressed || hovered ? { backgroundColor: theme.colors.surfaceRaised } : null]}
            >
              <View style={[styles.rowIcon, { backgroundColor: theme.colors.surfaceRaised }]}>
                <Icon name="workflow" size={15} tone="textMuted" />
              </View>
              <AppText>Workers</AppText>
              <View style={styles.rowValue}>
                <View style={styles.marks}>
                  {workerLabels.slice(0, 3).map((agent) => <AgentMark key={agent.key} agent={agent.key} size={16} />)}
                </View>
                <AppText tone="textMuted" numberOfLines={1} style={styles.rowValueText}>{workerSummary}</AppText>
              </View>
              <Icon name="chevron-right" size={14} tone="textSubtle" />
            </Control>
            {hairline}
            <SelectPopover
              row={{ icon: "layers" }}
              label="Cost"
              caption="MODELS"
              options={COORDINATION_COSTS.map((entry) => ({ id: entry.id, label: entry.label, hint: entry.hint }))}
              value={cost}
              onChange={setCost}
              open={menu === "cost"}
              onOpenChange={(open) => setMenu(open ? "cost" : null)}
            />
          </View>
        </View>

        <Button label="Start coordination" loading={busy} disabled={busy || !ready} onPress={() => void start()} style={styles.cta} />
      </ScrollScreen>
      <WorkersSheet
        visible={sheet === "workers"}
        agents={workerChoices}
        lead={chosenLead}
        selected={workers}
        onChange={setWorkers}
        onClose={() => setSheet(null)}
      />
      <IssuesSheet
        visible={sheet === "issues"}
        rpc={rpc}
        project={chosenProject}
        linked={issues}
        onChange={setIssues}
        onClose={() => setSheet(null)}
      />
    </View>
  );
}

const styles = StyleSheet.create({
  frame: { flex: 1, minHeight: 0 },
  content: { gap: spacing.lg, paddingTop: spacing.lg },
  padded: { gap: 12, padding: 16 },
  intro: { gap: 4 },
  composer: { marginHorizontal: -12 },
  section: { gap: spacing.sm },
  sectionCaption: { paddingHorizontal: 4 },
  card: { borderWidth: 1, borderRadius: 14, overflow: "hidden" },
  cardNote: { padding: 14 },
  hairline: { height: StyleSheet.hairlineWidth, marginLeft: 54 },
  rowTrigger: { minHeight: 52, flexDirection: "row", alignItems: "center", gap: 12, paddingHorizontal: 14 },
  rowIcon: { width: 28, height: 28, borderRadius: 8, alignItems: "center", justifyContent: "center" },
  rowValue: { flex: 1, minWidth: 0, flexDirection: "row", alignItems: "center", justifyContent: "flex-end", gap: 6 },
  rowValueText: { flexShrink: 1, textAlign: "right" },
  marks: { flexDirection: "row", gap: 2 },
  cta: { alignSelf: "stretch", minHeight: 48 },
  actions: { flexDirection: "row", justifyContent: "flex-end", gap: spacing.sm },
});
