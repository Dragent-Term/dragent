import { useMemo, useState } from "react";
import { SectionList, StyleSheet, View } from "react-native";
import { router } from "expo-router";
import { AgentMark, AppText, EmptyState, StatusBadge } from "../../components";
import { Icon } from "../../components/Icon";
import { SearchField } from "../../components/SearchField";
import { OptionRow, SelectPopover } from "../../components";

const ANY_AGENT = "__any__";
import { ResourceRow, countLabel } from "../../components/ResourceRow";
import { useTheme } from "../../theme";
import { spacing } from "../../theme/tokens";
import type { RelayCatalog, RelayTab } from "../../transport/relayCatalog";
import { countNeeds, mostUrgent, needLabel, needsSummary, statusDot, tabNeed, type NeedCounts } from "../../machines/attention";
import { machineCoordinationDetailPath, machineTabPath, machineWorkspacePath } from "../../machines/routes";
import { usePinnedWorkspaces } from "../../machines/pins";
import { useWorkspaceActions } from "./WorkspaceActions";
import {
  DIRECTORY_SORTS,
  PINNED_SECTION,
  directoryTeams,
  type DirectoryTeam,
  STATUS_FILTERS,
  directoryAgents,
  workspaceDirectory,
  type DirectorySort,
  type DirectoryWorkspace,
  type StatusFilter,
} from "../../machines/workspaceDirectory";

/** Agents shown on a row before "+N". */
const MAX_MARKS = 3;

function agentName(kind: string): string {
  const known: Record<string, string> = {
    claude: "Claude Code", codex: "Codex", opencode: "OpenCode", gemini: "Gemini",
    dragent: "Dragent", cursor: "Cursor", copilot: "Copilot", amp: "Amp", qwen: "Qwen",
  };
  return known[kind.toLowerCase()] ?? kind;
}

/** "Codex · Fix auth needs your input": what the workspace waits on. */
function waitingLine(workspace: DirectoryWorkspace): string | null {
  const tab = workspace.tabs.find((candidate) => candidate.id === workspace.openTabId);
  if (!tab) return null;
  const need = tabNeed(tab);
  if (!need) return null;
  const who = tab.agent_kind ? `${agentName(tab.agent_kind)} · ` : "";
  const what = need === "needs_input" ? "needs your input" : need === "failed" ? "failed" : "finished · ready to review";
  return `${who}${tab.title || "Tab"} ${what}`;
}

function agentKinds(tabs: readonly RelayTab[]): string[] {
  const kinds: string[] = [];
  for (const tab of tabs) {
    const kind = (tab.agent_kind ?? "").trim().toLowerCase();
    if (kind && !kinds.includes(kind)) kinds.push(kind);
  }
  return kinds;
}

function WorkspaceRow({ deviceId, workspace, onActions }: {
  deviceId: string;
  workspace: DirectoryWorkspace;
  onActions: (workspace: DirectoryWorkspace) => void;
}) {
  const { theme } = useTheme();
  const waiting = waitingLine(workspace);
  const urgent = mostUrgent(workspace.needs);
  const kinds = agentKinds(workspace.tabs);
  const working = workspace.needs.working;
  const detail = waiting
    ?? [workspace.branch, countLabel(workspace.tabs.length, "tab", "tabs"), working > 0 ? `${working} working` : null]
      .filter(Boolean)
      .join(" · ");
  const summary = needsSummary(workspace.needs);
  return (
    <ResourceRow
      icon="branch"
      dot={statusDot(workspace.needs)}
      title={workspace.name}
      detail={detail}
      accessibilityLabel={`${workspace.name}, branch ${workspace.branch}${summary ? `, ${summary}` : ""}. Opens ${workspace.openTabId === null ? "the workspace" : "its tab"}.`}
      accessibilityHint="Long-press to pin or delete it"
      onLongPress={() => onActions(workspace)}
      onPress={() =>
        router.push(
          workspace.openTabId === null
            ? machineWorkspacePath(deviceId, workspace.key)
            : machineTabPath(deviceId, workspace.openTabId),
        )
      }
      trailing={
        urgent ? (
          <StatusBadge label={needLabel(urgent, workspace.needs)} tone={urgent === "review" ? "success" : "danger"} />
        ) : kinds.length > 0 ? (
          <View style={styles.marks} accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
            {kinds.slice(0, MAX_MARKS).map((kind, index) => (
              <View
                key={kind}
                style={[
                  styles.mark,
                  { marginLeft: index === 0 ? 0 : -6, borderColor: theme.colors.background, backgroundColor: theme.colors.surface },
                ]}
              >
                <AgentMark agent={kind} size={16} />
              </View>
            ))}
            {kinds.length > MAX_MARKS ? (
              <AppText variant="caption" tone="textMuted" style={styles.more}>+{kinds.length - MAX_MARKS}</AppText>
            ) : null}
          </View>
        ) : undefined
      }
    />
  );
}

/**
 * One coordination (an agent team): its lead and workers' workspaces as a
 * single row, marked as coordination work, opening the coordination.
 */
function TeamRow({ deviceId, team }: { deviceId: string; team: DirectoryTeam }) {
  const { theme } = useTheme();
  const urgent = mostUrgent(team.needs);
  const working = team.needs.working;
  const detail = [
    "Coordination",
    countLabel(team.workspaces.length, "workspace", "workspaces"),
    working > 0 ? `${working} working` : null,
  ].filter(Boolean).join(" · ");
  const summary = needsSummary(team.needs);
  return (
    <ResourceRow
      icon="workflow"
      dot={statusDot(team.needs)}
      title={team.title}
      detail={detail}
      accessibilityLabel={`Coordination ${team.title}, ${countLabel(team.workspaces.length, "workspace", "workspaces")}${summary ? `, ${summary}` : ""}. Opens the coordination.`}
      onPress={() => router.push(machineCoordinationDetailPath(deviceId, team.id))}
      trailing={
        urgent ? (
          <StatusBadge label={needLabel(urgent, team.needs)} tone={urgent === "review" ? "success" : "danger"} />
        ) : team.agents.length > 0 ? (
          <View style={styles.marks} accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
            {team.agents.slice(0, MAX_MARKS).map((kind, index) => (
              <View
                key={kind}
                style={[
                  styles.mark,
                  { marginLeft: index === 0 ? 0 : -6, borderColor: theme.colors.background, backgroundColor: theme.colors.surface },
                ]}
              >
                <AgentMark agent={kind} size={16} />
              </View>
            ))}
          </View>
        ) : undefined
      }
    />
  );
}

type ListRow =
  | { kind: "team"; key: string; team: DirectoryTeam }
  | { kind: "workspace"; key: string; workspace: DirectoryWorkspace };

interface ListSection {
  key: string;
  title: string;
  /** Its workspaces, a coordination's included. */
  count: number;
  needs: NeedCounts;
  data: ListRow[];
}

/**
 * Every workspace on the desktop in one list, grouped under its project and
 * searchable. A workspace opens straight onto its tab (the one waiting for
 * you first); the project level is a heading, not a screen to go through.
 */
export function WorkspaceDirectory({
  deviceId,
  catalog,
  header,
  initialStatus = "all",
}: {
  deviceId: string;
  catalog: RelayCatalog;
  header?: React.ReactNode;
  /** Where the status filter starts (home's "3 ready to review" opens it on "To review"). */
  initialStatus?: StatusFilter;
}) {
  const { theme } = useTheme();
  const [query, setQuery] = useState("");
  const [status, setStatus] = useState<StatusFilter>(initialStatus);
  const [agent, setAgent] = useState<string>(ANY_AGENT);
  const [sort, setSort] = useState<DirectorySort>("attention");
  const [menu, setMenu] = useState<"status" | "agent" | "sort" | null>(null);
  const agents = useMemo(() => directoryAgents(catalog), [catalog]);
  const chosenAgent = agents.includes(agent) ? agent : ANY_AGENT;
  const filtered = query.trim() !== "" || status !== "all" || chosenAgent !== ANY_AGENT;
  const pinned = usePinnedWorkspaces(deviceId);
  const actions = useWorkspaceActions(deviceId, pinned);
  const sections = useMemo(
    () =>
      workspaceDirectory(catalog, query, { status, agent: chosenAgent === ANY_AGENT ? null : chosenAgent, sort, pinned })
        .map((section) => ({ ...section, data: section.data.filter((workspace) => !actions.hidden.has(workspace.key)) }))
        .filter((section) => section.data.length > 0 || section.key !== PINNED_SECTION),
    [catalog, query, status, chosenAgent, sort, pinned, actions.hidden],
  );
  const total = catalog.workspaces.length;
  // Coordination work shows once per coordination, as one row at the top of
  // its project, while the list isn't searched or filtered (then its
  // workspaces show among the rest).
  const listed = useMemo((): ListSection[] => {
    const teams = filtered ? [] : directoryTeams(catalog, pinned);
    const out: ListSection[] = sections.map((section) => {
      const own = section.key === PINNED_SECTION ? [] : teams.filter((team) => team.projectKey === section.key);
      return {
        ...section,
        count: section.data.length + own.reduce((sum, team) => sum + team.workspaces.length, 0),
        data: [
          ...own.map((team): ListRow => ({ kind: "team", key: `team:${team.id}`, team })),
          ...section.data.map((workspace): ListRow => ({ kind: "workspace", key: workspace.key, workspace })),
        ],
      };
    });
    // A coordination whose project isn't listed goes with the other workspaces.
    const placed = new Set(out.map((section) => section.key));
    const homeless = teams.filter((team) => !placed.has(team.projectKey) || team.projectKey === PINNED_SECTION);
    if (homeless.length > 0) {
      const rows = homeless.map((team): ListRow => ({ kind: "team", key: `team:${team.id}`, team }));
      const other = out.find((section) => section.key === "");
      if (other) other.data.unshift(...rows);
      else out.push({ key: "", title: "Other workspaces", count: homeless.reduce((sum, team) => sum + team.workspaces.length, 0), needs: countNeeds(homeless.flatMap((team) => team.workspaces.flatMap((workspace) => workspace.tabs))), data: rows });
    }
    return out.filter((section) => section.data.length > 0 || section.key !== PINNED_SECTION);
  }, [sections, filtered, catalog, pinned]);
  return (
    <SectionList
      sections={listed}
      keyExtractor={(row) => row.key}
      style={styles.list}
      contentContainerStyle={styles.content}
      keyboardShouldPersistTaps="handled"
      keyboardDismissMode="on-drag"
      stickySectionHeadersEnabled
      ListHeaderComponent={
        <View style={styles.top}>
          {header}
          {total > 0 ? (
            <SearchField
              label="Search workspaces"
              placeholder={`Search ${countLabel(total, "workspace", "workspaces")}, branches, tabs…`}
              value={query}
              onChangeText={setQuery}
              clearButtonMode="while-editing"
            />
          ) : null}
          {total > 0 ? (
            <OptionRow accessibilityLabel="Filter and sort workspaces">
              <SelectPopover
                label="Status"
                caption="SHOW"
                options={STATUS_FILTERS.map((entry) => ({ id: entry.id, label: entry.label }))}
                value={status}
                onChange={setStatus}
                open={menu === "status"}
                onOpenChange={(open) => setMenu(open ? "status" : null)}
              />
              {agents.length > 0 ? (
                <SelectPopover
                  label="Agent"
                  caption="AGENT"
                  options={[{ id: ANY_AGENT, label: "Any agent" }, ...agents.map((kind) => ({ id: kind, label: agentName(kind) }))]}
                  value={chosenAgent}
                  onChange={setAgent}
                  open={menu === "agent"}
                  onOpenChange={(open) => setMenu(open ? "agent" : null)}
                />
              ) : null}
              <SelectPopover
                label="Sort"
                caption="SORT BY"
                options={DIRECTORY_SORTS.map((entry) => ({ id: entry.id, label: entry.label }))}
                value={sort}
                onChange={setSort}
                open={menu === "sort"}
                onOpenChange={(open) => setMenu(open ? "sort" : null)}
              />
            </OptionRow>
          ) : null}
        </View>
      }
      renderSectionHeader={({ section }) => {
        const urgent = mostUrgent(section.needs);
        return (
          <View style={[styles.sectionHeader, { backgroundColor: theme.colors.background }]}>
            <View style={styles.sectionName}>
              {section.key === PINNED_SECTION ? <Icon name="pin" size={13} tone="textMuted" /> : null}
              <AppText variant="caption" style={styles.sectionTitle} numberOfLines={1}>
                {section.title}
              </AppText>
            </View>
            <AppText variant="caption" tone={urgent === "needs_input" || urgent === "failed" ? "danger" : "textSubtle"}>
              {urgent ? needLabel(urgent, section.needs) : countLabel(section.count, "workspace", "workspaces")}
            </AppText>
          </View>
        );
      }}
      renderItem={({ item }) =>
        item.kind === "team"
          ? <TeamRow deviceId={deviceId} team={item.team} />
          : <WorkspaceRow deviceId={deviceId} workspace={item.workspace} onActions={actions.open} />}
      ListFooterComponent={<>{actions.sheet}</>}
      ListEmptyComponent={
        filtered ? (
          <EmptyState title="No matching workspaces" message="Try other words, or show all workspaces." />
        ) : (
          <EmptyState
            title="No workspaces shared yet"
            message="Workspaces on this desktop appear here, grouped by project, as soon as it shares them."
          />
        )
      }
    />
  );
}

const styles = StyleSheet.create({
  list: { flex: 1, minHeight: 0 },
  content: { paddingHorizontal: spacing.md, paddingBottom: spacing.xl },
  top: { gap: spacing.sm, paddingTop: spacing.sm, paddingBottom: spacing.xs },
  sectionHeader: {
    flexDirection: "row",
    alignItems: "center",
    justifyContent: "space-between",
    gap: spacing.sm,
    paddingTop: spacing.md,
    paddingBottom: spacing.xs,
  },
  sectionName: { flexDirection: "row", alignItems: "center", gap: 6, flexShrink: 1 },
  sectionTitle: { fontWeight: "600", flexShrink: 1, textTransform: "uppercase", letterSpacing: 0.4 },
  marks: { flexDirection: "row", alignItems: "center" },
  mark: { width: 24, height: 24, borderRadius: 12, borderWidth: 1.5, alignItems: "center", justifyContent: "center" },
  more: { marginLeft: 4 },
});
