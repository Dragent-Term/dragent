/**
 * Shared agent-coordination presentation. Pure UI only: no PreviewContext,
 * no rpc, no mutations. Preview (sample, with its own controls) and live
 * (readonly) render the same layouts through these rows/views so the four
 * themes and compact responsive structure stay identical.
 */
import { StyleSheet, View } from "react-native";
import { AppText, Control, Icon, StatusBadge, type StatusTone } from "../components";
import { useTheme } from "../theme";
import { radii, spacing } from "../theme/tokens";
import type { CoordinationTaskStatus, CoordinationTeamStatus } from "./types";

export const COORDINATION_STATUS_META: Record<CoordinationTeamStatus, { label: string; tone: StatusTone }> = {
  planning: { label: "Planning", tone: "working" },
  running: { label: "Running", tone: "working" },
  done: { label: "Done", tone: "success" },
  stopped: { label: "Stopped", tone: "neutral" },
};

export type CoordinationFilter = "all" | "active" | "finished";

export const COORDINATION_FILTER_OPTIONS = [
  { id: "all", label: "All", hint: "All coordinations" },
  { id: "active", label: "Active", hint: "Planning or running coordinations" },
  { id: "finished", label: "Finished", hint: "Done or stopped coordinations" },
] as const;

export function matchesCoordinationFilter(status: CoordinationTeamStatus, filter: CoordinationFilter): boolean {
  if (filter === "active") return status === "planning" || status === "running";
  if (filter === "finished") return status === "done" || status === "stopped";
  return true;
}

export const COORDINATION_TASK_META: Record<CoordinationTaskStatus, { label: string; tone: StatusTone }> = {
  pending: { label: "Pending", tone: "neutral" },
  starting: { label: "Starting", tone: "working" },
  running: { label: "Running", tone: "working" },
  blocked: { label: "Blocked", tone: "warning" },
  done: { label: "Done", tone: "success" },
  failed: { label: "Failed", tone: "warning" },
  needs_decision: { label: "Needs decision", tone: "warning" },
  cancelled: { label: "Cancelled", tone: "neutral" },
};

export interface CoordinationRowModel {
  readonly key: string;
  readonly id: string;
  readonly title: string;
  readonly goal: string;
  readonly meta: string;
  readonly doneCount: number;
  readonly totalCount: number;
  readonly status: CoordinationTeamStatus;
  /** The team is waiting on the person. */
  readonly needsYou?: boolean;
}

/** One coordination as a card: title and status, project, plan progress, counts. */
export function CoordinationListRow({
  row,
  onOpen,
}: {
  row: CoordinationRowModel;
  onOpen: (id: string) => void;
}) {
  const { theme } = useTheme();
  const status = COORDINATION_STATUS_META[row.status];
  const share = row.totalCount > 0 ? Math.min(1, row.doneCount / row.totalCount) : 0;
  return (
    <Control
      accessibilityRole="button"
      accessibilityLabel={`${row.title}, ${status.label}, ${row.doneCount} of ${row.totalCount} done${row.needsYou ? ", needs you" : ""}`}
      onPress={() => onOpen(row.id)}
      style={({ pressed, hovered }) => [
        styles.card,
        {
          borderColor: row.needsYou ? theme.colors.warning : theme.colors.border,
          backgroundColor: pressed || hovered ? theme.colors.surfaceRaised : theme.colors.surface,
        },
      ]}
    >
      <View style={styles.cardTop}>
        <View style={[styles.icon, { backgroundColor: theme.colors.surfaceRaised, borderColor: theme.colors.border }]}>
          <Icon name="agent" size={18} tone="text" />
        </View>
        <View style={styles.copy}>
          <AppText style={styles.name} numberOfLines={2}>{row.title}</AppText>
          {row.goal ? (
            <AppText variant="caption" tone="textMuted" numberOfLines={1}>{row.goal}</AppText>
          ) : null}
        </View>
        <Icon name="chevron-right" size={16} />
      </View>
      <View style={styles.badges}>
        <StatusBadge label={status.label} tone={status.tone} />
        {row.needsYou ? <StatusBadge label="Needs you" tone="warning" /> : null}
        <AppText variant="caption" tone="textMuted" style={styles.count}>
          {row.totalCount > 0 ? `${row.doneCount} of ${row.totalCount} done` : "Planning"}
        </AppText>
      </View>
      {row.totalCount > 0 ? (
        <View style={[styles.track, { backgroundColor: theme.colors.surfaceRaised }]}>
          <View style={[styles.fill, { width: `${share * 100}%`, backgroundColor: theme.colors.success }]} />
        </View>
      ) : null}
      {row.meta ? <AppText variant="caption" tone="textMuted" numberOfLines={1}>{row.meta}</AppText> : null}
    </Control>
  );
}

export interface CoordinationTaskModel {
  readonly key: string;
  readonly title: string;
  readonly meta: string;
  readonly status: CoordinationTaskStatus;
}

/** One plan task row (readonly): title, status word, dependency/result meta. */
export function CoordinationTaskRow({ task }: { task: CoordinationTaskModel }) {
  const { theme } = useTheme();
  const status = COORDINATION_TASK_META[task.status];
  return (
    <View style={[styles.row, { borderColor: theme.colors.border }]}>
      <View style={[styles.icon, { backgroundColor: theme.colors.surfaceRaised, borderColor: theme.colors.border }]}>
        <Icon name="tasks" size={18} tone="text" />
      </View>
      <View style={styles.copy}>
        <AppText style={styles.name} numberOfLines={3}>{task.title}</AppText>
        <AppText variant="caption" tone="textMuted" numberOfLines={2}>{task.meta}</AppText>
      </View>
      <View style={styles.trailing}>
        <StatusBadge label={status.label} tone={status.tone} />
      </View>
    </View>
  );
}

export interface CoordinationMessageModel {
  readonly key: string;
  readonly header: string;
  readonly body: string;
  readonly accessibilityLabel: string;
}

/** One message row (readonly): header plus body. */
export function CoordinationMessageRow({ message }: { message: CoordinationMessageModel }) {
  const { theme } = useTheme();
  return (
    <View
      accessible
      accessibilityLabel={message.accessibilityLabel}
      style={[styles.messageRow, { borderColor: theme.colors.border }]}
    >
      <AppText variant="caption" tone="textMuted" numberOfLines={1}>{message.header}</AppText>
      <AppText>{message.body}</AppText>
    </View>
  );
}

/** Limited-activity notice; callers render it only on truncated replies. */
export function CoordinationTruncatedNotice({ label = "Showing limited recent activity." }: { label?: string }) {
  return (
    <AppText variant="caption" tone="textMuted" numberOfLines={2}>
      {label}
    </AppText>
  );
}

const styles = StyleSheet.create({
  row: {
    minHeight: 64,
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
  trailing: { flexDirection: "row", alignItems: "center", gap: spacing.md, marginLeft: "auto" },
  name: { fontWeight: "600" },
  card: { borderWidth: 1, borderRadius: radii.xl, borderCurve: "continuous", padding: spacing.md, gap: spacing.sm },
  cardTop: { flexDirection: "row", alignItems: "center", gap: spacing.md },
  badges: { flexDirection: "row", flexWrap: "wrap", alignItems: "center", gap: spacing.sm },
  count: { marginLeft: "auto" },
  track: { height: 4, borderRadius: 2, overflow: "hidden" },
  fill: { height: 4, borderRadius: 2 },
  messageRow: { borderBottomWidth: 1, gap: 2, paddingVertical: spacing.sm, paddingHorizontal: spacing.xs },
});
