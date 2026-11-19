import type { ReactNode } from "react";
import { StyleSheet, View, type ViewProps } from "react-native";
import { spacing } from "../theme/tokens";
import { AppText } from "./AppText";

export interface PageHeaderProps extends ViewProps {
  title: string;
  subtitle?: string | undefined;
  /** Optional context, styled quietly rather than as a promotional eyebrow. */
  eyebrow?: string | undefined;
  titleVariant?: "hero" | "title" | "body";
  actions?: ReactNode;
}

export function PageHeader({
  title, subtitle, eyebrow, titleVariant = "title", actions, children, style, ...props
}: PageHeaderProps) {
  return (
    <View {...props} style={[styles.shell, style]}>
      <View style={styles.row}>
        <View style={styles.titles}>
          {eyebrow ? <AppText variant="caption" tone="textMuted">{eyebrow}</AppText> : null}
          <AppText variant={titleVariant} accessibilityRole="header" style={styles.title}>{title}</AppText>
          {subtitle ? <AppText variant="caption" tone="textMuted">{subtitle}</AppText> : null}
        </View>
        {actions ? <View style={styles.actions}>{actions}</View> : null}
      </View>
      {children}
    </View>
  );
}

const styles = StyleSheet.create({
  shell: { gap: spacing.md },
  row: { flexDirection: "row", flexWrap: "wrap", alignItems: "center", gap: spacing.md },
  titles: { flexGrow: 1, flexShrink: 1, flexBasis: 180, minWidth: 0, gap: spacing.xs },
  title: { fontWeight: "600" },
  actions: { maxWidth: "100%", flexDirection: "row", flexWrap: "wrap", alignItems: "center", gap: spacing.sm },
});
