import type { ReactNode } from "react";
import { StyleSheet, View } from "react-native";
import { spacing } from "../theme/tokens";
import { AppText } from "./AppText";
import { Button } from "./Button";

export interface EmptyStateProps {
  /** @deprecated Kept for call-site compatibility; use the icon slot instead. */
  glyph?: string;
  icon?: ReactNode;
  title: string;
  message: string;
  actionLabel?: string;
  onAction?: () => void;
  children?: ReactNode;
}

export function EmptyState({ icon, title, message, actionLabel, onAction, children }: EmptyStateProps) {
  return (
    <View style={styles.shell}>
      <View style={styles.content}>
        {icon ? (
          <View
            accessible={false}
            accessibilityElementsHidden
            importantForAccessibility="no-hide-descendants"
            style={styles.icon}
          >{icon}</View>
        ) : null}
        <AppText accessibilityRole="header" style={styles.title}>{title}</AppText>
        <AppText tone="textMuted" style={styles.centered}>{message}</AppText>
        {actionLabel && onAction ? <Button label={actionLabel} onPress={onAction} variant="ghost" /> : null}
        {children}
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  shell: { alignItems: "center", paddingVertical: spacing.xxl, paddingHorizontal: spacing.lg },
  content: { width: "100%", maxWidth: 360, alignItems: "center", gap: spacing.sm },
  icon: { marginBottom: spacing.xs },
  title: { textAlign: "center", fontWeight: "600", maxWidth: "100%" },
  centered: { textAlign: "center", maxWidth: "100%" },
});
