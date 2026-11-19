import { Pressable, StyleSheet, View } from "react-native";
import { router } from "expo-router";
import { AgentMark, AppText, BrowserMark, Icon, TerminalMark } from "../../components";
import { useTheme } from "../../theme";
import { radii, spacing } from "../../theme/tokens";
import { machineTabPath } from "../../machines/routes";
import { ageLabel, useResumePoint } from "../../machines/resume";
import { pairedDesktopName } from "../../transport/e2ee/native";

/**
 * "Pick up where you left off": the last tab opened on this phone, one tap
 * away. Hidden until there is one, and for a desktop no longer paired.
 */
export function ResumeCard() {
  const { theme } = useTheme();
  const point = useResumePoint();
  if (!point || !pairedDesktopName(point.deviceId)) return null;
  const where = [point.context, point.desktopName].filter(Boolean).join(" · ");
  return (
    <Pressable
      onPress={() => router.push(machineTabPath(point.deviceId, point.tabId))}
      accessibilityRole="button"
      accessibilityLabel={`Resume ${point.title}${where ? `, ${where}` : ""}`}
      style={({ pressed }) => [
        styles.card,
        {
          borderColor: theme.colors.border,
          backgroundColor: pressed ? theme.colors.surfaceRaised : theme.colors.surface,
        },
      ]}
    >
      <View style={[styles.icon, { backgroundColor: theme.colors.surfaceRaised, borderColor: theme.colors.border }]}>
        {point.mark === "browser" ? (
          <BrowserMark size={22} />
        ) : point.mark === "terminal" ? (
          <TerminalMark size={26} />
        ) : point.mark ? (
          <AgentMark agent={point.mark} size={24} />
        ) : (
          <Icon name="terminal" size={18} tone="textMuted" />
        )}
      </View>
      <View style={styles.text}>
        <AppText variant="caption" tone="textMuted">Resume · {ageLabel(point.at)}</AppText>
        <AppText variant="body" style={styles.title} numberOfLines={1}>{point.title}</AppText>
        {where ? <AppText variant="caption" tone="textSubtle" numberOfLines={1}>{where}</AppText> : null}
      </View>
      <Icon name="chevron-right" size={18} tone="textSubtle" />
    </Pressable>
  );
}

const styles = StyleSheet.create({
  card: {
    flexDirection: "row",
    alignItems: "center",
    gap: spacing.md,
    padding: spacing.md,
    borderWidth: 1,
    borderRadius: radii.lg,
  },
  icon: { width: 40, height: 40, borderRadius: 12, borderWidth: 1, alignItems: "center", justifyContent: "center" },
  text: { flex: 1, minWidth: 0, gap: 2 },
  title: { fontWeight: "600" },
});
