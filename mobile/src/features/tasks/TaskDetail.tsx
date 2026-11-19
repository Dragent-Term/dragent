import { useState } from "react";
import { StyleSheet, View } from "react-native";
import {
  AppText,
  Avatar,
  Button,
  Card,
  EmptyState,
  StatusBadge,
} from "../../components";
import { ScrollScreen } from "../../layout";
import { useTheme } from "../../theme";
import { spacing } from "../../theme/tokens";
import type { PreviewSnapshot } from "../../preview/types";
import { Feedback } from "../shared/Feedback";
import {
  CREATE_WORKSPACE_PREVIEW_MESSAGE,
  TASK_STATUS,
  selectPreviewTask,
  taskProjectName,
  taskReference,
  timeAgo,
} from "./taskSamples";

export { CREATE_WORKSPACE_PREVIEW_MESSAGE };

/**
 * One sample task: status, description, discussion placeholder and a
 * preview-only Create-workspace action. The action is a no-op with honest
 * messaging, matching other preview actions: clearly labelled preview-only,
 * nothing leaves the device, nothing is created.
 */
export function TaskDetail({
  snapshot,
  desktopId,
  taskId,
}: {
  snapshot: PreviewSnapshot;
  desktopId: string;
  taskId: string;
}) {
  const { theme } = useTheme();
  const [feedback, setFeedback] = useState<string | null>(null);

  const task = selectPreviewTask(snapshot, desktopId, taskId);
  if (!task) {
    return (
      <ScrollScreen maxWidth={1040} contentStyle={styles.content}>
        <EmptyState
          title="Task unavailable"
          message="This sample task is not part of that machine."
        />
      </ScrollScreen>
    );
  }

  const status = TASK_STATUS[task.status];
  const project = taskProjectName(snapshot, task.projectId);

  return (
    <ScrollScreen maxWidth={1040} contentStyle={styles.content}>
      <AppText variant="caption" tone="textSubtle">
        Sample task · changes stay in this preview.
      </AppText>
      <View style={styles.header}>
        <View style={styles.identityRow}>
          <StatusBadge label={status.label} tone={status.tone} />
          <AppText variant="caption" tone="textMuted" numberOfLines={1} style={styles.identity}>
            {`${project} · ${taskReference(task)}`}
          </AppText>
        </View>
        <AppText variant="title" accessibilityRole="header" style={styles.title}>
          {task.title}
        </AppText>
        <View style={styles.byline}>
          <Avatar name={task.author} size={16} />
          <AppText variant="caption" tone="textMuted" numberOfLines={1} style={styles.bylineText}>
            {`${task.author} · ${timeAgo(task.updatedAt)}`}
          </AppText>
        </View>
      </View>
      <Card title="Status" subtitle="Sample state only — nothing syncs.">
        <View style={styles.statusRow}>
          <StatusBadge label={status.label} tone={status.tone} />
          <AppText variant="caption" tone="textMuted" style={styles.statusCopy}>
            {task.status === "open"
              ? "Open in this preview. Closing is disabled here."
              : "Closed in this sample set. Reopening is disabled here."}
          </AppText>
        </View>
      </Card>
      <Card title="Description">
        <AppText selectable>{task.description}</AppText>
      </Card>
      <Card
        title="Discussion"
        subtitle="Read-only placeholder — sample tasks carry no comments."
      >
        <EmptyState
          title="No discussion yet"
          message="Comments on sample tasks are not part of this preview. Nothing is posted anywhere."
        />
      </Card>
      <Card
        title="Workspace"
        subtitle="Preview-only — no workspace is created."
      >
        <AppText variant="caption" tone="textMuted">
          Starting real work from a task needs a workspace. Creation stays disabled in this local preview.
        </AppText>
        <View style={styles.actions}>
          <Button
            label="Create workspace (preview)"
            size="sm"
            variant="ghost"
            onPress={() => setFeedback(CREATE_WORKSPACE_PREVIEW_MESSAGE)}
          />
        </View>
        <Feedback message={feedback} />
      </Card>
      <View
        accessible
        accessibilityLabel="Local preview notice"
        style={[styles.notice, { borderColor: theme.colors.border, backgroundColor: theme.colors.surfaceRaised }]}
      >
        <AppText variant="caption" tone="textMuted" style={styles.noticeText}>
          Local preview using sample data. No provider was contacted.
        </AppText>
      </View>
    </ScrollScreen>
  );
}

const styles = StyleSheet.create({
  content: { gap: spacing.md },
  header: { gap: spacing.sm, minWidth: 0 },
  identityRow: { flexDirection: "row", alignItems: "center", gap: spacing.sm, minWidth: 0 },
  identity: { flex: 1, minWidth: 0 },
  title: { fontWeight: "700" },
  byline: { flexDirection: "row", alignItems: "center", gap: 6, flexShrink: 1, minWidth: 0 },
  bylineText: { flexShrink: 1, minWidth: 0 },
  statusRow: { flexDirection: "row", flexWrap: "wrap", alignItems: "center", gap: spacing.sm },
  statusCopy: { flexGrow: 1, flexShrink: 1, flexBasis: 160, minWidth: 0 },
  actions: { flexDirection: "row", flexWrap: "wrap", gap: spacing.sm },
  notice: {
    flexDirection: "row",
    alignItems: "center",
    gap: spacing.sm,
    borderWidth: 1,
    borderRadius: 10,
    borderCurve: "continuous",
    paddingVertical: spacing.sm,
    paddingHorizontal: spacing.md,
    minWidth: 0,
  },
  noticeText: { flex: 1, minWidth: 0 },
});
