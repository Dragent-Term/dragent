import type { ReactNode } from "react";
import { Modal, Pressable, StyleSheet, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { useTheme } from "../theme";
import { radii, spacing } from "../theme/tokens";
import { AppText } from "./AppText";
import { Control } from "./Control";
import { Icon, type IconName } from "./Icon";
import { ThemedStatusBar } from "./ThemedStatusBar";

export interface SheetAction {
  readonly label: string;
  readonly detail?: string | undefined;
  readonly icon: IconName;
  /** Red, for actions that can't be undone. */
  readonly destructive?: boolean | undefined;
  readonly disabled?: boolean | undefined;
  readonly onPress: () => void;
}

/**
 * A bottom sheet of actions for one thing (a workspace, a tab): its name
 * and a short line about it on top, then the actions. Tapping outside
 * closes it; choosing an action closes it first.
 */
export function ActionSheet({ visible, title, subtitle, header, actions, onClose }: {
  visible: boolean;
  title: string;
  subtitle?: string | undefined;
  /** Shown beside the title (an agent mark, a status dot). */
  header?: ReactNode | undefined;
  actions: readonly SheetAction[];
  onClose: () => void;
}) {
  const { theme } = useTheme();
  const insets = useSafeAreaInsets();
  return (
    <Modal visible={visible} animationType="fade" transparent onRequestClose={onClose}>
      <ThemedStatusBar />
      <View style={styles.scrim}>
        <Pressable accessibilityRole="button" accessibilityLabel="Close" style={StyleSheet.absoluteFill} onPress={onClose} />
        <View
          style={[
            styles.sheet,
            { backgroundColor: theme.colors.surface, borderColor: theme.colors.border, paddingBottom: Math.max(insets.bottom, spacing.sm) },
          ]}
        >
          <View style={[styles.grabber, { backgroundColor: theme.colors.border }]} />
          <View style={styles.head}>
            {header}
            <View style={styles.flex}>
              <AppText style={styles.title} numberOfLines={1}>{title}</AppText>
              {subtitle ? <AppText variant="caption" tone="textMuted" numberOfLines={2}>{subtitle}</AppText> : null}
            </View>
          </View>
          <View style={[styles.rule, { backgroundColor: theme.colors.border }]} />
          {actions.map((action) => (
            <Control
              key={action.label}
              accessibilityRole="button"
              accessibilityLabel={action.detail ? `${action.label}, ${action.detail}` : action.label}
              disabled={action.disabled}
              onPress={() => {
                onClose();
                action.onPress();
              }}
              style={({ pressed, hovered }) => [
                styles.row,
                pressed || hovered ? { backgroundColor: theme.colors.surfaceRaised } : null,
                action.disabled ? styles.disabled : null,
              ]}
            >
              <View style={styles.mark}>
                <Icon name={action.icon} size={18} tone={action.destructive ? "danger" : "textMuted"} />
              </View>
              <View style={styles.flex}>
                <AppText tone={action.destructive ? "danger" : "text"}>{action.label}</AppText>
                {action.detail ? <AppText variant="caption" tone="textSubtle" numberOfLines={2}>{action.detail}</AppText> : null}
              </View>
            </Control>
          ))}
        </View>
      </View>
    </Modal>
  );
}

const styles = StyleSheet.create({
  scrim: { flex: 1, justifyContent: "flex-end", backgroundColor: "rgba(0,0,0,0.45)" },
  sheet: {
    borderTopWidth: StyleSheet.hairlineWidth,
    borderTopLeftRadius: radii.xl,
    borderTopRightRadius: radii.xl,
    paddingTop: spacing.sm,
  },
  grabber: { alignSelf: "center", width: 36, height: 4, borderRadius: 2, marginBottom: spacing.xs },
  head: { flexDirection: "row", alignItems: "center", gap: spacing.md, paddingHorizontal: spacing.lg, paddingVertical: spacing.sm },
  title: { fontWeight: "600" },
  rule: { height: StyleSheet.hairlineWidth, marginHorizontal: spacing.lg, marginBottom: spacing.xs },
  row: { minHeight: 52, flexDirection: "row", alignItems: "center", gap: spacing.md, paddingHorizontal: spacing.lg, paddingVertical: 6 },
  mark: { width: 22, alignItems: "center" },
  flex: { flex: 1, minWidth: 0 },
  disabled: { opacity: 0.45 },
});
