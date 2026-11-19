import { StyleSheet, View } from "react-native";
import { AppText, Control, Icon, type IconName } from "../../components";
import { useTheme } from "../../theme";
import { spacing } from "../../theme/tokens";

export interface ActionRowProps {
  icon: IconName;
  title: string;
  detail?: string;
  onPress: () => void;
}

/**
 * One quiet navigation row for quick access and account actions.
 * Title plus optional detail; the caller owns where it goes.
 */
export function ActionRow({ icon, title, detail, onPress }: ActionRowProps) {
  const { theme } = useTheme();
  return (
    <Control
      accessibilityRole="button"
      accessibilityLabel={detail ? `${title}. ${detail}` : title}
      onPress={onPress}
      style={({ pressed, hovered }) => [
        styles.row,
        {
          borderColor: theme.colors.border,
          backgroundColor: pressed || hovered ? theme.colors.surfaceRaised : theme.colors.background,
        },
      ]}
    >
      <Icon name={icon} size={18} tone="textMuted" />
      <View style={styles.copy}>
        <AppText style={styles.title} numberOfLines={2}>{title}</AppText>
        {detail ? <AppText variant="caption" tone="textMuted" numberOfLines={2}>{detail}</AppText> : null}
      </View>
      <Icon name="chevron-right" size={16} />
    </Control>
  );
}

const styles = StyleSheet.create({
  row: {
    minHeight: 56,
    borderBottomWidth: 1,
    flexDirection: "row",
    alignItems: "center",
    gap: spacing.md,
    paddingVertical: spacing.sm,
    paddingHorizontal: spacing.xs,
  },
  copy: { flex: 1, minWidth: 0, gap: 2 },
  title: { fontWeight: "500" },
});
