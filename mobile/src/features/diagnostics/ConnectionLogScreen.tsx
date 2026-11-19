import { useState } from "react";
import { FlatList, StyleSheet, View } from "react-native";
import * as Clipboard from "expo-clipboard";
import { AppText, Button, EmptyState } from "../../components";
import { confirmAction } from "../../components/confirmAction";
import { useTheme } from "../../theme";
import { radii, spacing } from "../../theme/tokens";
import { pairedDesktopName } from "../../transport/e2ee/native";
import { clearConnectionLog, useConnectionLog } from "../../diagnostics/connectionLog";
import { formatLog, type LogEntry } from "../../diagnostics/connectionLogModel";

const nameOf = (desktop: string) => pairedDesktopName(desktop) ?? "Desktop";

const TIPS = [
  "Make sure Dragent is open on the desktop and the computer is awake and online.",
  "On the desktop's screen here, use Refresh to connect again.",
  "On the desktop, check Settings → Mobile still lists this phone. If not, pair again.",
  "Still stuck? Copy this log into a bug report: it holds connection states and errors only, never your messages, code or keys.",
];

function timeOf(at: number): string {
  return new Date(at).toLocaleString(undefined, { month: "short", day: "numeric", hour: "numeric", minute: "2-digit", second: "2-digit" });
}

/** What happened to the phone's connections, newest first, to copy when something won't connect. */
export function ConnectionLogScreen() {
  const { theme } = useTheme();
  const log = useConnectionLog();
  const [copied, setCopied] = useState(false);
  const tone = (entry: LogEntry) => (entry.level === "error" ? theme.colors.danger : entry.level === "warn" ? theme.colors.warning : theme.colors.success);

  return (
    <FlatList
      data={log}
      keyExtractor={(entry, index) => `${entry.at}:${index}`}
      contentContainerStyle={styles.content}
      ListHeaderComponent={
        <View style={styles.top}>
          <View style={[styles.tips, { backgroundColor: theme.colors.surface, borderColor: theme.colors.border }]}>
            <AppText style={styles.strong}>If a desktop won't connect</AppText>
            {TIPS.map((tip, index) => (
              <View key={tip} style={styles.tip}>
                <AppText variant="caption" tone="accent" style={styles.number}>{index + 1}</AppText>
                <AppText variant="caption" tone="textMuted" style={styles.grow}>{tip}</AppText>
              </View>
            ))}
          </View>
          {log.length > 0 ? (
            <View style={styles.actions}>
              <Button
                label={copied ? "Copied" : "Copy log"}
                size="sm"
                onPress={() =>
                  void Clipboard.setStringAsync(formatLog(log, nameOf)).then(() => {
                    setCopied(true);
                    setTimeout(() => setCopied(false), 1500);
                  })
                }
              />
              <Button
                label="Clear"
                size="sm"
                variant="ghost"
                onPress={() =>
                  confirmAction({ title: "Clear the connection log?", message: "It starts again from now.", actionLabel: "Clear", actionStyle: "destructive", onConfirm: clearConnectionLog })
                }
              />
            </View>
          ) : null}
        </View>
      }
      renderItem={({ item }) => (
        <View style={[styles.row, { borderColor: theme.colors.border }]}>
          <View style={[styles.dot, { backgroundColor: tone(item) }]} />
          <View style={styles.grow}>
            <AppText variant="caption" tone="textSubtle">
              {`${timeOf(item.at)} · ${item.desktop ? nameOf(item.desktop) : "App"}${item.count > 1 ? ` · ×${item.count}` : ""}`}
            </AppText>
            <AppText selectable>{item.text}</AppText>
          </View>
        </View>
      )}
      ListEmptyComponent={<EmptyState title="Nothing logged yet" message="Connections to your desktops are noted here as they happen." />}
    />
  );
}

const styles = StyleSheet.create({
  content: { padding: spacing.md, paddingBottom: spacing.xl },
  top: { gap: spacing.md, paddingBottom: spacing.sm },
  tips: { borderWidth: 1, borderRadius: radii.lg, borderCurve: "continuous", padding: spacing.md, gap: spacing.sm },
  strong: { fontWeight: "600" },
  tip: { flexDirection: "row", gap: spacing.sm },
  number: { fontWeight: "700", width: 14 },
  grow: { flex: 1, minWidth: 0 },
  actions: { flexDirection: "row", gap: spacing.sm },
  row: { flexDirection: "row", alignItems: "flex-start", gap: spacing.sm, paddingVertical: spacing.sm, borderBottomWidth: StyleSheet.hairlineWidth },
  dot: { width: 8, height: 8, borderRadius: 4, marginTop: 6 },
});
