import { ActivityIndicator, StyleSheet, View } from "react-native";
import { useTheme } from "../theme";
import { spacing } from "../theme/tokens";
import { AppText } from "./AppText";

export interface LoadingStateProps {
  label?: string;
}

/** Common loading feedback for screens fetching or restoring real data. */
export function LoadingState({ label = "Loading…" }: LoadingStateProps) {
  const { theme } = useTheme();
  return (
    <View
      accessible
      accessibilityRole="progressbar"
      accessibilityLabel={label}
      accessibilityState={{ busy: true }}
      style={styles.shell}
    >
      <ActivityIndicator accessible={false} color={theme.colors.textMuted} />
      <AppText variant="caption" tone="textMuted" style={styles.label}>{label}</AppText>
    </View>
  );
}

const styles = StyleSheet.create({
  shell: { alignItems: "center", justifyContent: "center", gap: spacing.md, padding: spacing.xl },
  label: { textAlign: "center" },
});
