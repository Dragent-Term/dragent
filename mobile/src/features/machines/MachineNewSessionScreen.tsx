import { useEffect, useMemo, useState } from "react";
import { ActivityIndicator, Platform, StyleSheet, TextInput, View } from "react-native";
import { router } from "expo-router";
import { AgentMark, AppText, Button, Control, Icon, LoadingState, SelectPopover } from "../../components";
import { inputColorsFor } from "../../theme/inputColors";
import { ScrollScreen } from "../../layout";
import { useTheme } from "../../theme";
import { spacing } from "../../theme/tokens";
import { useMachine } from "../../machines/MachineContext";
import { machinePath, machineProjectPath, machineWorkspacePath } from "../../machines/routes";
import { projectKey, workspaceKey, workspacesForProject } from "../../machines/machineCatalog";
import { readGitBranches } from "../../transport/relayRpcMethods";
import { coordinationOptions } from "../../coordination/rpc";
import { MAX_SESSION_PROMPT, startSession } from "../../sessions/rpc";
import { MachineHeader } from "./MachineHeader";
import { Feedback } from "../shared/Feedback";
import { useStartOptions } from "./StartPickers";

const NEW = "__new__";
const DEFAULT = "__default__";

type Menu = "project" | "workspace" | "branch" | "agent" | "model" | null;

/**
 * A new agent session on the paired desktop: one of the agents installed
 * there works on a task, in a new workspace of a project (from its default
 * branch or one chosen) or in a workspace that's already there. The desktop
 * starts it through its own new-workspace prompt.
 */
export function MachineNewSessionScreen({
  deviceId,
  project: initialProject,
  workspace: initialWorkspace,
}: {
  deviceId: string;
  project?: string;
  workspace?: string;
}) {
  const { theme } = useTheme();
  const machine = useMachine();
  const rpc = machine.rpc;
  const catalog = machine.catalog.status === "live" ? machine.catalog.catalog : null;
  const { options, error: loadError, reload } = useStartOptions(rpc, coordinationOptions);
  const [project, setProject] = useState<string | null>(initialProject ?? null);
  const [workspace, setWorkspace] = useState<string>(initialWorkspace ?? NEW);
  const [branch, setBranch] = useState<string>(DEFAULT);
  const [branches, setBranches] = useState<readonly string[]>([]);
  const [agent, setAgent] = useState<string | null>(null);
  const [model, setModel] = useState<string>(DEFAULT);
  const [prompt, setPrompt] = useState("");
  const [menu, setMenu] = useState<Menu>(null);
  const [busy, setBusy] = useState(false);
  const [focused, setFocused] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const projects = options?.projects ?? [];
  const chosenProject = project ?? projects[0]?.path ?? null;
  // The catalog's workspaces of that project, by folder.
  const catalogProject = catalog?.projects.find((entry) => entry.path === chosenProject) ?? null;
  const workspaces = useMemo(
    () => (catalog && catalogProject ? workspacesForProject(catalog, catalogProject) : []),
    [catalog, catalogProject],
  );
  const chosenWorkspace = workspace === NEW || workspaces.some((entry) => entry.path === workspace) ? workspace : NEW;
  const agents = options?.agents ?? [];
  const chosenAgent = agents.find((entry) => entry.key === agent) ?? agents.find((entry) => entry.key !== "dragent") ?? agents[0] ?? null;
  const chosenModel = chosenAgent?.models.some((entry) => entry.id === model) ? model : DEFAULT;

  // A new workspace's base: the project's branches.
  useEffect(() => {
    setBranch(DEFAULT);
    setBranches([]);
    if (!rpc || !chosenProject) return;
    let cancelled = false;
    readGitBranches(rpc, chosenProject).then(
      (result) => !cancelled && setBranches(result.branches),
      () => {},
    );
    return () => {
      cancelled = true;
    };
  }, [rpc, chosenProject]);

  const back = machinePath(deviceId);
  const leave = () => (router.canGoBack() ? router.back() : router.navigate({ pathname: back.pathname, params: back.params }));
  const header = (
    <MachineHeader
      title="New agent session"
      subtitle={catalog?.machine?.name ?? (machine.catalog.status === "live" ? machine.catalog.machine.name : "Desktop")}
      online={machine.catalog.status === "live" ? machine.catalog.machine.online : null}
      onBack={leave}
      backLabel="Back"
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

  const ready = rpc !== null && !!chosenProject && !!chosenAgent && prompt.trim().length > 0;
  const start = async () => {
    if (!ready || !rpc || !chosenProject || !chosenAgent) return;
    setBusy(true);
    setError(null);
    try {
      const result = await startSession(rpc, {
        project: chosenProject,
        agent: chosenAgent.key,
        prompt,
        model: chosenModel === DEFAULT ? null : chosenModel,
        workspace: chosenWorkspace === NEW ? null : chosenWorkspace,
        branch: chosenWorkspace === NEW && branch !== DEFAULT ? branch : null,
      });
      if (!result.started) {
        setError(result.reason ?? "The desktop didn't start it.");
        return;
      }
      // Where the session shows up: its workspace, or the project, where a
      // new workspace appears at the top while its agent works.
      const existing = workspaces.find((entry) => entry.path === chosenWorkspace);
      if (existing) router.replace(machineWorkspacePath(deviceId, workspaceKey(existing)));
      else if (catalogProject) router.replace(machineProjectPath(deviceId, projectKey(catalogProject)));
      else router.replace({ pathname: back.pathname, params: back.params });
    } catch {
      setError("The desktop didn't answer. Check it's online, then try again.");
    } finally {
      setBusy(false);
    }
  };

  const machineName = catalog?.machine?.name ?? (machine.catalog.status === "live" ? machine.catalog.machine.name : "the desktop");
  const projectLabel = projects.find((entry) => entry.path === chosenProject)?.label ?? "";
  const where = chosenWorkspace === NEW
    ? `A new workspace of ${projectLabel || "the project"} on ${machineName}.`
    : `${workspaces.find((entry) => entry.path === chosenWorkspace)?.name ?? "This workspace"} on ${machineName}.`;
  const caret = inputColorsFor(theme.colors.accent, theme.colors.background, Platform.OS);
  const hairline = <View style={[styles.hairline, { backgroundColor: theme.colors.border }]} />;

  return (
    <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
      {header}
      <ScrollScreen maxWidth={720} contentStyle={styles.content}>
        <View style={styles.intro}>
          <AppText variant="title">What should the agent work on?</AppText>
          <AppText variant="caption" tone="textMuted">{where}</AppText>
        </View>

        <View
          style={[
            styles.composer,
            { backgroundColor: theme.colors.surface, borderColor: focused ? theme.colors.accent : theme.colors.border },
          ]}
        >
          <TextInput
            accessibilityLabel="Task"
            placeholder="Describe the task, the way you would to a teammate…"
            placeholderTextColor={theme.colors.textSubtle}
            value={prompt}
            onChangeText={setPrompt}
            onFocus={() => setFocused(true)}
            onBlur={() => setFocused(false)}
            multiline
            maxLength={MAX_SESSION_PROMPT}
            editable={!busy}
            textAlignVertical="top"
            selectionColor={caret.selection}
            cursorColor={caret.caret}
            style={[styles.input, { color: theme.colors.text }]}
          />
          <View style={styles.composerBar}>
            <View style={styles.composerPickers}>
              {agents.length > 0 && chosenAgent ? (
                <SelectPopover
                  pill
                  label="Agent"
                  caption="INSTALLED ON THE DESKTOP"
                  options={agents.map((entry) => ({
                    id: entry.key,
                    label: entry.label,
                    leading: <AgentMark agent={entry.key} size={16} />,
                  }))}
                  value={chosenAgent.key}
                  onChange={(next) => {
                    setAgent(next);
                    setModel(DEFAULT);
                  }}
                  open={menu === "agent"}
                  onOpenChange={(open) => setMenu(open ? "agent" : null)}
                  searchable={agents.length > 8}
                  searchPlaceholder="Find an agent"
                />
              ) : null}
              {chosenAgent && chosenAgent.models.length > 0 ? (
                <SelectPopover
                  pill
                  label="Model"
                  caption="MODEL"
                  options={[{ id: DEFAULT, label: "Default model" }, ...chosenAgent.models.map((entry) => ({ id: entry.id, label: entry.label }))]}
                  value={chosenModel}
                  onChange={setModel}
                  open={menu === "model"}
                  onOpenChange={(open) => setMenu(open ? "model" : null)}
                  searchable={chosenAgent.models.length > 8}
                  searchPlaceholder="Find a model"
                />
              ) : null}
            </View>
            <Control
              accessibilityRole="button"
              accessibilityLabel="Start session"
              accessibilityState={{ disabled: busy || !ready, busy }}
              disabled={busy || !ready}
              onPress={() => void start()}
              style={({ pressed }) => [
                styles.send,
                {
                  backgroundColor: ready ? theme.colors.accent : theme.colors.surfaceRaised,
                  opacity: pressed ? 0.85 : 1,
                },
              ]}
            >
              {busy ? (
                <ActivityIndicator size="small" color={theme.colors.onAccent} />
              ) : (
                <Icon name="arrow-up" size={18} tone={ready ? "onAccent" : "textSubtle"} />
              )}
            </Control>
          </View>
        </View>
        {agents.length === 0 ? (
          <AppText variant="caption" tone="textMuted">No agents are installed on the desktop. Install one there first.</AppText>
        ) : null}
        <Feedback message={error} />

        <View style={styles.section}>
          <AppText variant="micro" tone="textSubtle" style={styles.sectionCaption}>WHERE IT RUNS</AppText>
          {projects.length === 0 ? (
            <AppText tone="textMuted">No projects are open on the desktop. Open one there first.</AppText>
          ) : (
            <View style={[styles.card, { backgroundColor: theme.colors.surface, borderColor: theme.colors.border }]}>
              <SelectPopover
                row={{ icon: "folder" }}
                label="Project"
                caption="PROJECT"
                options={projects.map((entry) => ({ id: entry.path, label: entry.label, hint: entry.path }))}
                value={chosenProject ?? projects[0]!.path}
                onChange={(next) => {
                  setProject(next);
                  setWorkspace(NEW);
                }}
                open={menu === "project"}
                onOpenChange={(open) => setMenu(open ? "project" : null)}
                searchable={projects.length > 8}
                searchPlaceholder="Find a project"
              />
              {hairline}
              <SelectPopover
                row={{ icon: "grid" }}
                label="Workspace"
                caption="WORK IN"
                options={[
                  { id: NEW, label: "New workspace", hint: "Own branch and folder" },
                  ...workspaces.map((entry) => ({ id: entry.path, label: entry.name, hint: entry.branch ?? entry.path })),
                ]}
                value={chosenWorkspace}
                onChange={setWorkspace}
                open={menu === "workspace"}
                onOpenChange={(open) => setMenu(open ? "workspace" : null)}
                searchable={workspaces.length > 8}
                searchPlaceholder="Find a workspace"
              />
              {chosenWorkspace === NEW && branches.length > 0 ? (
                <>
                  {hairline}
                  <SelectPopover
                    row={{ icon: "branch" }}
                    label="Start from"
                    caption="BRANCH FROM"
                    options={[{ id: DEFAULT, label: "Default branch" }, ...branches.map((name) => ({ id: name, label: name }))]}
                    value={branch}
                    onChange={setBranch}
                    open={menu === "branch"}
                    onOpenChange={(open) => setMenu(open ? "branch" : null)}
                    searchable={branches.length > 8}
                    searchPlaceholder="Find a branch"
                  />
                </>
              ) : null}
            </View>
          )}
        </View>

        <Button
          label="Start session"
          loading={busy}
          disabled={busy || !ready}
          onPress={() => void start()}
          style={styles.cta}
        />
      </ScrollScreen>
    </View>
  );
}

const styles = StyleSheet.create({
  frame: { flex: 1, minHeight: 0 },
  content: { gap: spacing.lg, paddingTop: spacing.lg },
  padded: { gap: 12, padding: 16 },
  intro: { gap: 4 },
  composer: { borderWidth: 1, borderRadius: 16, paddingTop: 12, paddingHorizontal: 14, paddingBottom: 10, gap: 8 },
  input: { minHeight: 112, maxHeight: 260, fontSize: 16, lineHeight: 23, padding: 0 },
  composerBar: { flexDirection: "row", alignItems: "center", gap: spacing.sm },
  composerPickers: { flex: 1, minWidth: 0, flexDirection: "row", alignItems: "center", gap: 6 },
  send: { width: 36, height: 36, borderRadius: 18, alignItems: "center", justifyContent: "center" },
  section: { gap: spacing.sm },
  sectionCaption: { paddingHorizontal: 4 },
  card: { borderWidth: 1, borderRadius: 14, overflow: "hidden" },
  hairline: { height: StyleSheet.hairlineWidth, marginLeft: 54 },
  cta: { alignSelf: "stretch", minHeight: 48 },
});
