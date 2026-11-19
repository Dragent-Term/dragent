import { useEffect, useMemo } from "react";
import { SectionList, StyleSheet, View } from "react-native";
import { router } from "expo-router";
import { AppText, Button, Control, DragentMark, EmptyState } from "../../components";
import { confirmAction } from "../../components/confirmAction";
import { useTheme } from "../../theme";
import { radii, spacing } from "../../theme/tokens";
import { machinePath } from "../../machines/routes";
import { pairedDesktopName } from "../../transport/e2ee/native";
import { clearNotificationHistory, markAllNotificationsRead, useNotificationHistory } from "../../notifications/history";
import type { NotificationEntry } from "../../notifications/historyModel";

function dayOf(at: number, now: Date): string {
  const day = (date: Date) => new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
  const days = Math.round((day(now) - day(new Date(at))) / 86_400_000);
  if (days <= 0) return "Today";
  if (days === 1) return "Yesterday";
  return new Date(at).toLocaleDateString(undefined, { weekday: days < 7 ? "long" : undefined, month: "short", day: "numeric" });
}

function timeOf(at: number): string {
  return new Date(at).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
}

/**
 * What this phone was told, newest first: every alert it received from your
 * desktops, kept on this phone only. Tapping one opens its desktop.
 */
export function NotificationHistoryScreen() {
  const { theme } = useTheme();
  const { entries } = useNotificationHistory();
  // Seen once this screen is open.
  useEffect(() => {
    markAllNotificationsRead();
  }, [entries.length]);
  const sections = useMemo(() => {
    const now = new Date();
    const out: { title: string; data: NotificationEntry[] }[] = [];
    for (const entry of entries) {
      const title = dayOf(entry.at, now);
      const last = out[out.length - 1];
      if (last && last.title === title) last.data.push(entry);
      else out.push({ title, data: [entry] });
    }
    return out;
  }, [entries]);

  return (
    <SectionList
      sections={sections}
      keyExtractor={(entry) => entry.id}
      contentContainerStyle={styles.content}
      stickySectionHeadersEnabled={false}
      ListHeaderComponent={
        entries.length > 0 ? (
          <View style={styles.top}>
            <AppText variant="caption" tone="textMuted" style={styles.grow}>Alerts from your desktops, kept on this phone only.</AppText>
            <Button
              label="Clear"
              size="sm"
              variant="ghost"
              onPress={() =>
                confirmAction({
                  title: "Clear all alerts?",
                  message: "They're removed from this list only.",
                  actionLabel: "Clear",
                  actionStyle: "destructive",
                  onConfirm: clearNotificationHistory,
                })
              }
            />
          </View>
        ) : undefined
      }
      renderSectionHeader={({ section }) => (
        <AppText variant="caption" tone="textSubtle" style={styles.heading}>{section.title.toUpperCase()}</AppText>
      )}
      renderItem={({ item }) => {
        const desktop = item.desktop ? pairedDesktopName(item.desktop) ?? "Desktop" : null;
        return (
          <Control
            accessibilityRole="button"
            accessibilityLabel={`${item.title}. ${item.body}`}
            onPress={() => (item.desktop ? router.push(machinePath(item.desktop)) : router.navigate("/"))}
            style={({ pressed }) => [styles.row, { backgroundColor: pressed ? theme.colors.surfaceRaised : theme.colors.surface, borderColor: theme.colors.border }]}
          >
            <View style={[styles.mark, { backgroundColor: theme.colors.surfaceRaised }]}>
              <DragentMark size={18} />
            </View>
            <View style={styles.grow}>
              <View style={styles.titleRow}>
                <AppText numberOfLines={1} style={[styles.title, styles.grow]}>{item.title}</AppText>
                <AppText variant="caption" tone="textSubtle">{timeOf(item.at)}</AppText>
              </View>
              {item.body ? <AppText variant="caption" tone="textMuted" numberOfLines={3}>{item.body}</AppText> : null}
              {desktop ? <AppText variant="caption" tone="textSubtle" numberOfLines={1}>{desktop}</AppText> : null}
            </View>
            {!item.read ? <View style={[styles.dot, { backgroundColor: theme.colors.accent }]} /> : null}
          </Control>
        );
      }}
      ItemSeparatorComponent={() => <View style={styles.gap} />}
      ListEmptyComponent={
        <EmptyState title="No alerts yet" message="When an agent needs you or finishes, the alert shows here too." />
      }
    />
  );
}

const styles = StyleSheet.create({
  content: { padding: spacing.md, paddingBottom: spacing.xl },
  top: { flexDirection: "row", alignItems: "center", gap: spacing.sm, paddingBottom: spacing.xs },
  heading: { letterSpacing: 0.6, paddingTop: spacing.md, paddingBottom: spacing.xs },
  row: { flexDirection: "row", alignItems: "flex-start", gap: spacing.sm, borderWidth: 1, borderRadius: radii.lg, borderCurve: "continuous", padding: spacing.sm },
  mark: { width: 32, height: 32, borderRadius: 9, alignItems: "center", justifyContent: "center" },
  grow: { flex: 1, minWidth: 0 },
  titleRow: { flexDirection: "row", alignItems: "center", gap: spacing.xs },
  title: { fontWeight: "600" },
  dot: { width: 8, height: 8, borderRadius: 4, marginTop: 6 },
  gap: { height: spacing.xs },
});
