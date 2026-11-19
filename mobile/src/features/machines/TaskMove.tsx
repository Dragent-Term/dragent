import { StyleSheet, View } from "react-native";
import { AppText, Button } from "../../components";
import { useTheme } from "../../theme";
import { spacing } from "../../theme/tokens";
import { TASK_STATUSES, TASK_STATUS_META, type TaskListFilter, type TaskStatus } from "../../tasks/types";

/** The statuses a filter shows, in board order. */
export function statusesFor(filter: TaskListFilter): readonly TaskStatus[] {
  return TASK_STATUSES.filter((key) =>
    filter === "all" ? true : filter === "closed" ? key === "done" || key === "canceled" : key !== "done" && key !== "canceled",
  );
}

/** "Move DRA-3 to": a chip per status, the way a card is dropped on a column. */
export function MoveBar({
  label,
  current,
  statuses,
  busy,
  onMove,
  onCancel,
}: {
  label: string;
  current: TaskStatus;
  statuses: readonly TaskStatus[];
  busy: boolean;
  onMove: (status: TaskStatus) => void;
  onCancel: () => void;
}) {
  const { theme } = useTheme();
  return (
    <View style={[styles.moveBar, { borderColor: theme.colors.accent, backgroundColor: theme.colors.surface }]}>
      <AppText style={styles.name} numberOfLines={1}>{label}</AppText>
      <View style={styles.chips}>
        {statuses.map((key) => (
          <Button
            key={key}
            label={TASK_STATUS_META[key].label}
            size="sm"
            variant={key === current ? "primary" : "ghost"}
            disabled={busy || key === current}
            onPress={() => onMove(key)}
          />
        ))}
        <Button label="Cancel" size="sm" variant="ghost" onPress={onCancel} />
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  name: { fontWeight: "600" },
  chips: { flexDirection: "row", flexWrap: "wrap", gap: spacing.xs },
  moveBar: { gap: spacing.xs, padding: spacing.sm, borderRadius: 10, borderWidth: 1 },
});
