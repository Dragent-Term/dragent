import { useState } from "react";
import { StyleSheet, View } from "react-native";
import { AppText, Button, Control, IconButton } from "../../components";
import { useTheme } from "../../theme";
import { spacing } from "../../theme/tokens";
import { dueDay, dueOn, monthGrid } from "../../tasks/types";

const WEEKDAYS = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];
const PRESETS = [
  { days: 0, label: "Today" },
  { days: 1, label: "Tomorrow" },
  { days: 7, label: "Next week" },
  { days: 30, label: "Next month" },
] as const;

/**
 * Any due date: quick choices, then a month calendar to pick any day
 * (‹ › move between months). Today is outlined, the due day filled.
 * `onPick(null)` clears it.
 */
export function DueDatePicker({
  value,
  onPick,
  disabled = false,
}: {
  value: number | null;
  onPick: (dueAt: number | null) => void;
  disabled?: boolean;
}) {
  const { theme } = useTheme();
  const start = new Date(value ?? Date.now());
  const [month, setMonth] = useState({ year: start.getFullYear(), month: start.getMonth() });
  const today = new Date();
  const due = value === null ? null : new Date(value);
  const same = (date: Date | null, day: number) =>
    date !== null && date.getFullYear() === month.year && date.getMonth() === month.month && date.getDate() === day;
  const shift = (delta: number) =>
    setMonth(({ year, month: m }) => {
      const index = year * 12 + m + delta;
      return { year: Math.floor(index / 12), month: ((index % 12) + 12) % 12 };
    });
  const title = new Date(month.year, month.month, 1).toLocaleDateString(undefined, { month: "long", year: "numeric" });

  return (
    <View style={[styles.frame, { borderColor: theme.colors.border, backgroundColor: theme.colors.surface }]}>
      <View style={styles.presets}>
        <Button label="No due date" size="sm" variant="ghost" disabled={disabled} onPress={() => onPick(null)} />
        {PRESETS.map((preset) => (
          <Button key={preset.label} label={preset.label} size="sm" variant="ghost" disabled={disabled} onPress={() => onPick(dueOn(preset.days))} />
        ))}
      </View>
      <View style={styles.header}>
        <IconButton name="arrow-left" label="Previous month" onPress={() => shift(-1)} />
        <AppText style={styles.title} accessibilityRole="header">{title}</AppText>
        <IconButton name="arrow-right" label="Next month" onPress={() => shift(1)} />
      </View>
      <View style={styles.week}>
        {WEEKDAYS.map((name) => (
          <AppText key={name} variant="caption" tone="textMuted" style={styles.weekday}>{name}</AppText>
        ))}
      </View>
      {monthGrid(month.year, month.month).map((week, row) => (
        <View key={row} style={styles.week}>
          {week.map((day, column) => {
            if (day === null) return <View key={column} style={styles.cell} />;
            const picked = same(due, day);
            const isToday = same(today, day);
            return (
              <Control
                key={column}
                accessibilityRole="button"
                accessibilityState={{ selected: picked, disabled }}
                accessibilityLabel={new Date(month.year, month.month, day).toLocaleDateString(undefined, { weekday: "long", month: "long", day: "numeric", year: "numeric" })}
                disabled={disabled}
                onPress={() => onPick(dueDay(month.year, month.month, day))}
                style={({ pressed, hovered }) => [
                  styles.cell,
                  styles.day,
                  {
                    backgroundColor: picked ? theme.colors.accent : pressed || hovered ? theme.colors.surfaceRaised : undefined,
                    borderColor: isToday && !picked ? theme.colors.accent : "transparent",
                  },
                ]}
              >
                <AppText tone={picked ? "onAccent" : "text"}>{String(day)}</AppText>
              </Control>
            );
          })}
        </View>
      ))}
    </View>
  );
}

const styles = StyleSheet.create({
  frame: { gap: spacing.xs, padding: spacing.sm, borderRadius: 12, borderWidth: StyleSheet.hairlineWidth, maxWidth: 420 },
  presets: { flexDirection: "row", flexWrap: "wrap", gap: spacing.xs },
  header: { flexDirection: "row", alignItems: "center", justifyContent: "space-between" },
  title: { fontWeight: "600" },
  week: { flexDirection: "row" },
  weekday: { flex: 1, textAlign: "center" },
  cell: { flex: 1, aspectRatio: 1, maxHeight: 44 },
  day: { alignItems: "center", justifyContent: "center", borderRadius: 8, borderWidth: 1 },
});
