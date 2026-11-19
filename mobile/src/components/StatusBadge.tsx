import { StyleSheet, View } from "react-native";
import { useMemo } from "react";
import { statusMarkColors, useTheme } from "../theme";
import { radii, spacing } from "../theme/tokens";
import { AppText, type TextTone } from "./AppText";

export type StatusTone = "neutral" | "accent" | "success" | "warning" | "danger";

export interface StatusBadgeProps {
  label: string;
  tone?: StatusTone;
  accessibilityLabel?: string;
}

export function StatusBadge({ label, tone = "neutral", accessibilityLabel = label }: StatusBadgeProps) {
  const { theme } = useTheme();
  const marks = useMemo(() => statusMarkColors(theme.colors), [theme.colors]);
  const dotTone: TextTone = tone === "neutral" ? "textSubtle" : tone;
  // Status colours sharpened for this theme's background, in their own family.
  const dotColor =
    tone === "success" ? marks.success
    : tone === "warning" ? marks.warning
    : tone === "danger" ? marks.danger
    : theme.colors[dotTone];
  return (
    <View accessible accessibilityRole="text" accessibilityLabel={accessibilityLabel} style={styles.shell}>
      <View style={[styles.dot, { backgroundColor: dotColor }]} />
      <AppText
        variant="caption"
        tone={tone === "danger" ? "danger" : "textMuted"}
        style={tone === "danger" ? { color: marks.danger } : undefined}
      >
        {label}
      </AppText>
    </View>
  );
}

const styles = StyleSheet.create({
  shell: {
    alignSelf: "flex-start",
    maxWidth: "100%",
    flexDirection: "row",
    alignItems: "center",
    gap: spacing.sm,
  },
  dot: { width: 7, height: 7, borderRadius: radii.pill, flexShrink: 0 },
});
